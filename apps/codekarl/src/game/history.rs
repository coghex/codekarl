//! History storage, kept outside the deterministic simulation state.

use sha2::{Digest, Sha256};

const MAGIC: &[u8; 8] = b"CKHISTIX";
const FOOTER: &[u8; 8] = b"CKHISTEN";
const VERSION: u32 = 1;
const MAX_YEARS: usize = 9_999;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Index {
    pub campaign: u64,
    pub origin: (u16, u8),
    pub years: Vec<(u16, [u8; 32])>,
    pub unfinished: Vec<u8>,
}

impl Index {
    pub(super) fn encode(&self) -> Option<Vec<u8>> {
        if !(1..=9999).contains(&self.origin.0)
            || !(1..=12).contains(&self.origin.1)
            || self.years.len() > MAX_YEARS
            || self
                .years
                .iter()
                .any(|(year, _)| !(1..=9999).contains(year))
            || self.years.windows(2).any(|pair| pair[0].0 >= pair[1].0)
        {
            return None;
        }
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&self.campaign.to_le_bytes());
        out.extend_from_slice(&self.origin.0.to_le_bytes());
        out.push(self.origin.1);
        out.extend_from_slice(&u32::try_from(self.years.len()).ok()?.to_le_bytes());
        for (year, hash) in &self.years {
            out.extend_from_slice(&year.to_le_bytes());
            out.extend_from_slice(hash);
        }
        out.extend_from_slice(&u64::try_from(self.unfinished.len()).ok()?.to_le_bytes());
        out.extend_from_slice(&self.unfinished);
        let checksum = Sha256::digest(&out);
        out.extend_from_slice(&checksum);
        out.extend_from_slice(&u64::try_from(out.len()).ok()?.to_le_bytes());
        out.extend_from_slice(FOOTER);
        Some(out)
    }

    pub(super) fn decode(bytes: &[u8]) -> Option<Self> {
        let footer = bytes.len().checked_sub(16)?;
        if &bytes[footer + 8..] != FOOTER {
            return None;
        }
        let length = usize::try_from(u64::from_le_bytes(
            bytes[footer..footer + 8].try_into().ok()?,
        ))
        .ok()?;
        if length != footer || length < 67 {
            return None;
        }
        let hash_at = length.checked_sub(32)?;
        if Sha256::digest(&bytes[..hash_at]).as_slice() != &bytes[hash_at..length] {
            return None;
        }
        let mut r = Input {
            bytes: &bytes[..hash_at],
            pos: 0,
        };
        if r.take(8)? != MAGIC || r.u32()? != VERSION {
            return None;
        }
        let campaign = r.u64()?;
        let year = r.u16()?;
        let month = *r.take(1)?.first()?;
        if !(1..=9999).contains(&year) || !(1..=12).contains(&month) {
            return None;
        }
        let count = usize::try_from(r.u32()?).ok()?;
        if count > MAX_YEARS {
            return None;
        }
        let mut years = Vec::with_capacity(count);
        for _ in 0..count {
            let year = r.u16()?;
            let hash = r.take(32)?.try_into().ok()?;
            if !(1..=9999).contains(&year)
                || years.last().is_some_and(|(previous, _)| *previous >= year)
            {
                return None;
            }
            years.push((year, hash));
        }
        let length = usize::try_from(r.u64()?).ok()?;
        let unfinished = r.take(length)?.to_vec();
        if r.pos != r.bytes.len() {
            return None;
        }
        Some(Self {
            campaign,
            origin: (year, month),
            years,
            unfinished,
        })
    }
}

struct Input<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Input<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let result = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(result)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn index_round_trips_long_history_and_rejects_damage() {
        let index = Index {
            campaign: 42,
            origin: (768, 10),
            years: (768..2768).map(|y| (y, [7; 32])).collect(),
            unfinished: vec![3, 4, 5],
        };
        let bytes = index.encode().unwrap();
        assert!(bytes.len() > 65_536);
        assert_eq!(Index::decode(&bytes), Some(index));
        for end in [0, 1, 20, bytes.len() - 1] {
            assert_eq!(Index::decode(&bytes[..end]), None);
        }
        let mut corrupt = bytes.clone();
        corrupt[30] ^= 1;
        assert_eq!(Index::decode(&corrupt), None);
        let mut extra = bytes;
        extra.push(0);
        assert_eq!(Index::decode(&extra), None);
    }
}

/// A stable campaign lock; the lock file itself is never removed.
pub(super) struct CampaignLock {
    _file: std::fs::File,
}

impl CampaignLock {
    fn open(saves: &std::path::Path, campaign: u64) -> std::io::Result<std::fs::File> {
        let locks = saves.join("history").join("locks");
        std::fs::create_dir_all(&locks)?;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(locks.join(format!("{campaign:016x}.lock")))
    }

    pub(super) fn shared(saves: &std::path::Path, campaign: u64) -> std::io::Result<Self> {
        let file = Self::open(saves, campaign)?;
        file.lock_shared()?;
        Ok(Self { _file: file })
    }

    pub(super) fn exclusive(saves: &std::path::Path, campaign: u64) -> std::io::Result<Self> {
        let locks = saves.join("history").join("locks");
        std::fs::read_dir(&locks)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(locks.join(format!("{campaign:016x}.lock")))?;
        file.try_lock().map_err(std::io::Error::other)?;
        Ok(Self { _file: file })
    }
}

/// Read the bounded trailer prefix before taking a campaign lock.
pub(super) fn campaign_prefix(file: &mut std::fs::File) -> std::io::Result<Option<u64>> {
    use std::io::{Read, Seek, SeekFrom};
    let length = file.metadata()?.len();
    if length < 16 {
        return Ok(None);
    }
    file.seek(SeekFrom::End(-16))?;
    let mut footer = [0_u8; 16];
    file.read_exact(&mut footer)?;
    if &footer[8..] != FOOTER {
        return Ok(None);
    }
    let trailer_len = u64::from_le_bytes(footer[..8].try_into().expect("eight bytes"));
    if trailer_len < 67 || trailer_len > length - 16 {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(length - 16 - trailer_len))?;
    let mut prefix = [0_u8; 23];
    file.read_exact(&mut prefix)?;
    if &prefix[..8] != MAGIC
        || u32::from_le_bytes(prefix[8..12].try_into().expect("four bytes")) != VERSION
    {
        return Ok(None);
    }
    let year = u16::from_le_bytes(prefix[20..22].try_into().expect("two bytes"));
    if !(1..=9999).contains(&year) || !(1..=12).contains(&prefix[22]) {
        return Ok(None);
    }
    Ok(Some(u64::from_le_bytes(
        prefix[12..20].try_into().expect("eight bytes"),
    )))
}

#[cfg(test)]
mod lock_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn shared_games_exclude_collector_and_prefix_is_bounded() {
        let dir = std::env::temp_dir().join(format!("ck-history-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = CampaignLock::shared(&dir, 42).unwrap();
        let second = CampaignLock::shared(&dir, 42).unwrap();
        assert!(CampaignLock::exclusive(&dir, 42).is_err());
        drop(first);
        drop(second);
        let exclusive = CampaignLock::exclusive(&dir, 42).unwrap();
        drop(exclusive);
        let path = dir.join("save");
        let index = Index {
            campaign: 42,
            origin: (768, 10),
            years: vec![],
            unfinished: vec![],
        };
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"core save bytes").unwrap();
        file.write_all(&index.encode().unwrap()).unwrap();
        drop(file);
        let mut file = std::fs::File::open(path).unwrap();
        assert_eq!(campaign_prefix(&mut file).unwrap(), Some(42));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GoodRecord {
    pub id: String,
    pub price: u64,
    pub produced: u64,
    pub used: u64,
    pub lost: u64,
    pub produced_value: u128,
    pub used_value: u128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PopRecord {
    pub id: u64,
    pub place: String,
    pub kind: String,
    pub culture: String,
    pub religion: String,
    pub people: u64,
    pub workers: u64,
    pub dependents: u64,
    pub food_days: u64,
    pub births: u64,
    pub deaths: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PlaceRecord {
    pub id: String,
    pub tag: Option<String>,
    pub people: u64,
    pub workers: u64,
    pub dependents: u64,
    pub food_days: u64,
    pub births: u64,
    pub deaths: u64,
    pub goods: Vec<GoodRecord>,
    pub pops: Vec<PopRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MonthRecord {
    pub year: u16,
    pub month: u8,
    pub places: Vec<PlaceRecord>,
}

impl MonthRecord {
    pub(super) fn sample(
        rules: &ck_economy::Rules,
        economy: &ck_economy::EconomyState,
        year: u16,
        month: u8,
    ) -> Self {
        let places = economy
            .places
            .iter()
            .map(|place| {
                let pops: Vec<_> = place
                    .pops
                    .iter()
                    .enumerate()
                    .map(|(i, pop)| PopRecord {
                        id: pop.id,
                        place: place.id.clone(),
                        kind: pop.kind.clone(),
                        culture: pop.culture.clone(),
                        religion: pop.religion.clone(),
                        people: pop.workers.saturating_add(pop.dependents),
                        workers: pop.workers,
                        dependents: pop.dependents,
                        food_days: super::view::days_of(
                            super::view::food_centi(rules, place, i),
                            pop.workers.saturating_add(pop.dependents),
                        ),
                        births: pop.counters.last.births,
                        deaths: pop.counters.last.deaths,
                    })
                    .collect();
                let workers = pops
                    .iter()
                    .fold(0_u64, |n, pop| n.saturating_add(pop.workers));
                let dependents = pops
                    .iter()
                    .fold(0_u64, |n, pop| n.saturating_add(pop.dependents));
                let food = (0..place.pops.len()).fold(0_u64, |n, i| {
                    n.saturating_add(super::view::food_centi(rules, place, i))
                });
                let last = &place.counters.last;
                let goods = rules
                    .goods
                    .iter()
                    .enumerate()
                    .map(|(i, good)| GoodRecord {
                        id: good.id.clone(),
                        price: place.prices_milli.get(i).copied().unwrap_or_default(),
                        produced: last.produced.get(i).copied().unwrap_or_default(),
                        used: last.used_by_pops.get(i).copied().unwrap_or_default(),
                        lost: last.lost.get(i).copied().unwrap_or_default(),
                        produced_value: last
                            .produced_value_milli
                            .get(i)
                            .copied()
                            .unwrap_or_default(),
                        used_value: last
                            .used_by_pops_value_milli
                            .get(i)
                            .copied()
                            .unwrap_or_default(),
                    })
                    .collect();
                PlaceRecord {
                    id: place.id.clone(),
                    tag: None,
                    people: workers.saturating_add(dependents),
                    workers,
                    dependents,
                    food_days: super::view::days_of(food, workers.saturating_add(dependents)),
                    births: last.births,
                    deaths: last.deaths,
                    goods,
                    pops,
                }
            })
            .collect();
        Self {
            year,
            month,
            places,
        }
    }
}

macro_rules! record_canonical {
    ($name:ident { $( $field:ident : $ty:ty ),+ $(,)? }) => {
        impl ck_sim::Canonical for $name {
            fn encode(&self, out: &mut Vec<u8>) -> Result<(), ck_sim::CanonicalError> {
                $(self.$field.encode(out)?;)+
                Ok(())
            }
            fn decode(input: &mut ck_sim::Reader<'_>) -> Result<Self, ck_sim::CanonicalError> {
                Ok(Self { $($field: <$ty as ck_sim::Canonical>::decode(input)?,)+ })
            }
        }
    };
}
record_canonical!(GoodRecord {
    id: String,
    price: u64,
    produced: u64,
    used: u64,
    lost: u64,
    produced_value: u128,
    used_value: u128
});
record_canonical!(PopRecord {
    id: u64,
    place: String,
    kind: String,
    culture: String,
    religion: String,
    people: u64,
    workers: u64,
    dependents: u64,
    food_days: u64,
    births: u64,
    deaths: u64
});
record_canonical!(PlaceRecord { id: String, tag: Option<String>, people: u64, workers: u64, dependents: u64, food_days: u64, births: u64, deaths: u64, goods: Vec<GoodRecord>, pops: Vec<PopRecord> });
record_canonical!(MonthRecord { year: u16, month: u8, places: Vec<PlaceRecord> });

pub(super) fn encode_year(months: &[MonthRecord]) -> std::io::Result<Vec<u8>> {
    use ck_sim::Canonical;
    use std::io::Write;
    let mut result = b"CKHIST".to_vec();
    result.extend_from_slice(&1_u32.to_le_bytes());
    let mut compressed =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    let mut plain = Vec::new();
    ck_sim::encode_length(months.len(), &mut plain).map_err(std::io::Error::other)?;
    for month in months {
        month.encode(&mut plain).map_err(std::io::Error::other)?;
    }
    compressed.write_all(&plain)?;
    result.write_all(&compressed.finish()?)?;
    Ok(result)
}

pub(super) fn decode_year(bytes: &[u8]) -> Option<Vec<MonthRecord>> {
    use ck_sim::Canonical;
    use std::io::Read;
    if bytes.get(..10)? != b"CKHIST\x01\x00\x00\x00" {
        return None;
    }
    let mut plain = Vec::new();
    flate2::read::DeflateDecoder::new(&bytes[10..])
        .read_to_end(&mut plain)
        .ok()?;
    Vec::<MonthRecord>::from_bytes(&plain).ok()
}

#[cfg(test)]
mod year_tests {
    use super::*;

    #[test]
    fn compressed_year_preserves_keys_and_large_values() {
        let month = MonthRecord {
            year: 768,
            month: 10,
            places: vec![PlaceRecord {
                id: "cong".into(),
                tag: None,
                people: 18,
                workers: 10,
                dependents: 8,
                food_days: 22,
                births: 2,
                deaths: 1,
                goods: vec![GoodRecord {
                    id: "oats".into(),
                    price: 1000,
                    produced: 3,
                    used: 2,
                    lost: 1,
                    produced_value: u128::from(u64::MAX) + 1,
                    used_value: 2000,
                }],
                pops: vec![PopRecord {
                    id: 77,
                    place: "cong".into(),
                    kind: "peasant".into(),
                    culture: "irish".into(),
                    religion: "pagan".into(),
                    people: 18,
                    workers: 10,
                    dependents: 8,
                    food_days: 22,
                    births: 2,
                    deaths: 1,
                }],
            }],
        };
        let bytes = encode_year(std::slice::from_ref(&month)).unwrap();
        assert_eq!(decode_year(&bytes), Some(vec![month]));
        assert_eq!(decode_year(&bytes[..9]), None);
        let mut unknown = bytes;
        unknown[6] = 2;
        assert_eq!(decode_year(&unknown), None);
    }
}

struct PendingYear {
    path: std::path::PathBuf,
    bytes: Vec<u8>,
    task: Option<std::thread::JoinHandle<std::io::Result<()>>>,
}

impl PendingYear {
    fn start(&mut self) {
        let path = self.path.clone();
        let bytes = self.bytes.clone();
        self.task = Some(std::thread::spawn(move || {
            let parent = path
                .parent()
                .ok_or_else(|| std::io::Error::other("year has no parent"))?;
            std::fs::create_dir_all(parent)?;
            match super::saves::write_new_atomically(&path, &bytes) {
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if std::fs::read(&path)? == bytes {
                        Ok(())
                    } else {
                        Err(error)
                    }
                }
                other => other,
            }
        }));
    }
    fn wait(&mut self) -> bool {
        self.task
            .take()
            .is_some_and(|task| task.join().is_ok_and(|result| result.is_ok()))
    }
}

/// Running, non-deterministic history and its held shared lock.
pub(super) struct Recorder {
    index: Index,
    months: Vec<MonthRecord>,
    lock: Option<CampaignLock>,
    saves: std::path::PathBuf,
    pending: Vec<PendingYear>,
    start: std::time::SystemTime,
}

impl Recorder {
    #[cfg(test)]
    pub(super) fn months(&self) -> &[MonthRecord] {
        &self.months
    }
    pub(super) fn new(saves: &std::path::Path, campaign: u64, origin: (u16, u8)) -> Self {
        let lock = CampaignLock::shared(saves, campaign).ok();
        Self {
            index: Index {
                campaign,
                origin,
                years: Vec::new(),
                unfinished: Vec::new(),
            },
            months: Vec::new(),
            lock,
            saves: saves.to_owned(),
            pending: Vec::new(),
            start: std::time::SystemTime::now(),
        }
    }

    pub(super) fn record(&mut self, month: MonthRecord) {
        let year = month.year;
        let december = month.month == 12;
        self.months.push(month);
        if !december {
            return;
        }
        let Ok(bytes) = encode_year(&self.months) else {
            return;
        };
        self.months.clear();
        if self.lock.is_none() {
            return;
        }
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        let hex = hash_hex(&hash);
        let path = self
            .saves
            .join("history")
            .join(format!("{:016x}", self.index.campaign))
            .join(format!("{year:04}-{hex}.ckhist"));
        self.index.years.push((year, hash));
        let mut pending = PendingYear {
            path,
            bytes,
            task: None,
        };
        pending.start();
        self.pending.push(pending);
    }

    /// What the graph viewer reads: the history as it stands, immutable.
    pub(super) fn view(
        &self,
        revision: u64,
        horizon: Option<(u16, u8)>,
    ) -> super::history_view::View {
        super::history_view::View {
            revision,
            saves: self.saves.clone(),
            campaign: self.index.campaign,
            origin: self.index.origin,
            years: self.index.years.clone(),
            pending: self
                .pending
                .iter()
                .map(|pending| {
                    (
                        Sha256::digest(&pending.bytes).into(),
                        std::sync::Arc::new(pending.bytes.clone()),
                    )
                })
                .collect(),
            running: std::sync::Arc::new(self.months.clone()),
            horizon,
        }
    }

    pub(super) fn trailer(&mut self) -> Option<Vec<u8>> {
        self.pending.retain_mut(|pending| {
            if pending.wait() {
                return false;
            }
            pending.start();
            !pending.wait()
        });
        self.index.unfinished = encode_year(&self.months).ok()?;
        self.index.encode()
    }

    pub(super) fn collect_async(&self) {
        if self.lock.is_none() {
            return;
        }
        #[cfg(test)]
        COLLECTION_SCHEDULED.with(|count| count.set(count.get() + 1));
        #[cfg(test)]
        let done = COLLECTION_DONE.with(|hook| hook.borrow_mut().take());
        let saves = self.saves.clone();
        let start = self.start;
        std::thread::spawn(move || {
            let _ = collect(&saves, start);
            #[cfg(test)]
            if let Some(done) = done {
                done();
            }
        });
    }

    pub(super) fn inherit_start(&mut self, previous: &Self) {
        self.start = previous.start;
    }
}

#[cfg(test)]
thread_local! {
    pub(super) static COLLECTION_SCHEDULED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(super) static COLLECTION_DONE: std::cell::RefCell<Option<Box<dyn FnOnce() + Send>>> = std::cell::RefCell::new(None);
}

impl Drop for Recorder {
    fn drop(&mut self) {
        for pending in &mut self.pending {
            let _ = pending.wait();
        }
    }
}

#[cfg(test)]
mod recorder_tests {
    use super::*;

    #[test]
    fn changed_trailer_after_prefix_is_a_gap() {
        use std::io::{Seek, SeekFrom};
        let dir = crate::testing::Dir::new();
        let path = dir.0.join("save");
        let mut index = Index {
            campaign: 42,
            origin: (768, 10),
            years: vec![],
            unfinished: encode_year(&[]).unwrap(),
        };
        std::fs::write(&path, index.encode().unwrap()).unwrap();
        let mut file = std::fs::File::open(&path).unwrap();
        let handoff = Handoff::begin(&mut file, &dir.0);
        assert!(matches!(&handoff, Handoff::Locked(42, _)));
        index.campaign = 43;
        std::fs::write(&path, index.encode().unwrap()).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let date = ck_sim::Date {
            year: 770,
            month: 3,
            day: 1,
            hour: 0,
        };
        let mut recorder = handoff.finish(&mut file, &dir.0, date).unwrap();
        let result = Index::decode(&recorder.trailer().unwrap()).unwrap();
        assert_ne!(result.campaign, 42);
        assert_ne!(result.campaign, 43);
        assert_eq!(result.origin, (770, 3));
        assert!(result.years.is_empty());
        assert!(CampaignLock::exclusive(&dir.0, 42).is_ok());
    }

    fn month(year: u16, month: u8) -> MonthRecord {
        MonthRecord {
            year,
            month,
            places: Vec::new(),
        }
    }

    fn month_with_deaths(year: u16, month_number: u8, deaths: u64) -> MonthRecord {
        let mut result = month(year, month_number);
        result.places.push(PlaceRecord {
            id: "place".into(),
            tag: None,
            people: 10,
            workers: 10,
            dependents: 0,
            food_days: 0,
            births: 0,
            deaths,
            goods: vec![],
            pops: vec![PopRecord {
                id: 7,
                place: "place".into(),
                kind: "peasant".into(),
                culture: "culture".into(),
                religion: "religion".into(),
                people: 10,
                workers: 10,
                dependents: 0,
                food_days: 0,
                births: 0,
                deaths,
            }],
        });
        result
    }

    #[test]
    fn december_writes_a_content_addressed_year_and_save_waits() {
        let dir = std::env::temp_dir().join(format!("ck-history-year-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut recorder = Recorder::new(&dir, 42, (768, 10));
        recorder.record(month(768, 10));
        recorder.record(month(768, 11));
        recorder.record(month(768, 12));
        let trailer = Index::decode(&recorder.trailer().unwrap()).unwrap();
        assert_eq!(trailer.years.len(), 1);
        let (year, hash) = trailer.years[0];
        let hex = hash_hex(&hash);
        let path = dir
            .join("history/000000000000002a")
            .join(format!("{year:04}-{hex}.ckhist"));
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(<[u8; 32]>::from(Sha256::digest(&bytes)), hash);
        assert_eq!(
            decode_year(&bytes).unwrap(),
            vec![month(768, 10), month(768, 11), month(768, 12)]
        );
        assert!(decode_year(&trailer.unfinished).unwrap().is_empty());
        let mut damaged = bytes;
        damaged[10] ^= 1;
        std::fs::write(path, damaged).unwrap();
        assert!(read_year(&dir, 42, year, &hash).is_none());
        drop(recorder);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn detached_new_recorders_preserve_each_unavailable_lock_store_and_recover() {
        use std::os::unix::fs::PermissionsExt;
        for case in 0..3 {
            let dir = crate::testing::Dir::new();
            let root = dir.0.join("history");
            let locks = root.join("locks");
            match case {
                0 => std::fs::write(&root, b"blocked root").unwrap(),
                1 => {
                    std::fs::create_dir(&root).unwrap();
                    std::fs::write(&locks, b"blocked locks").unwrap();
                }
                _ => {
                    std::fs::create_dir_all(&locks).unwrap();
                    std::fs::set_permissions(&locks, std::fs::Permissions::from_mode(0o555))
                        .unwrap();
                }
            }
            let mut recorder = Recorder::new(&dir.0, 42, (768, 12));
            assert!(recorder.lock.is_none());
            COLLECTION_SCHEDULED.with(|count| count.set(0));
            recorder.collect_async();
            COLLECTION_SCHEDULED.with(|count| {
                assert_eq!(
                    count.get(),
                    0,
                    "a detached game must not schedule collection"
                );
            });
            recorder.record(month(768, 12));
            let index = Index::decode(&recorder.trailer().unwrap()).unwrap();
            assert_eq!(index.origin, (768, 12));
            assert!(index.years.is_empty());
            assert!(!root.join("000000000000002a").exists());
            drop(recorder);
            match case {
                0 => {
                    assert_eq!(std::fs::read(&root).unwrap(), b"blocked root");
                    std::fs::remove_file(&root).unwrap();
                }
                1 => {
                    assert_eq!(std::fs::read(&locks).unwrap(), b"blocked locks");
                    std::fs::remove_file(&locks).unwrap();
                }
                _ => {
                    assert_eq!(std::fs::read_dir(&locks).unwrap().count(), 0);
                    std::fs::set_permissions(&locks, std::fs::Permissions::from_mode(0o755))
                        .unwrap();
                }
            }
            let recovered = Recorder::new(&dir.0, 42, (768, 12));
            assert!(recovered.lock.is_some());
        }
    }

    #[test]
    fn detached_recorder_never_writes_a_year() {
        let dir = std::env::temp_dir().join(format!("ck-history-detached-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("history"), b"not a directory").unwrap();
        let mut recorder = Recorder::new(&dir, 42, (768, 12));
        recorder.record(month(768, 12));
        let index = Index::decode(&recorder.trailer().unwrap()).unwrap();
        assert!(index.years.is_empty());
        assert_eq!(
            std::fs::read(dir.join("history")).unwrap(),
            b"not a directory"
        );
        drop(recorder);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_year_write_is_retried_before_the_next_save() {
        let dir = std::env::temp_dir().join(format!("ck-history-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut recorder = Recorder::new(&dir, 42, (768, 12));
        let blocked = dir.join("history/000000000000002a");
        std::fs::write(&blocked, b"block directory").unwrap();
        recorder.record(month(768, 12));
        let index = Index::decode(&recorder.trailer().unwrap()).unwrap();
        assert_eq!(index.years.len(), 1);
        assert!(read_year(&dir, 42, 768, &index.years[0].1).is_none());
        std::fs::remove_file(blocked).unwrap();
        recorder.trailer().unwrap();
        assert_eq!(
            read_year(&dir, 42, 768, &index.years[0].1),
            Some(vec![month(768, 12)])
        );
        drop(recorder);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn older_save_restores_unfinished_months_and_branches_immutable_years() {
        use std::io::{Seek, SeekFrom, Write};
        let dir = std::env::temp_dir().join(format!("ck-history-branch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut original = Recorder::new(&dir, 42, (768, 10));
        original.record(month(768, 12));
        for m in 1..=3 {
            original.record(month(769, m));
        }
        let old = original.trailer().unwrap();
        original.record(month_with_deaths(769, 4, 1));
        original.record(month(769, 12));
        let future_a = Index::decode(&original.trailer().unwrap()).unwrap();
        let path = dir.join("old-save");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"core").unwrap();
        file.write_all(&old).unwrap();
        drop(file);
        let mut file = std::fs::File::open(path).unwrap();
        let handoff = Handoff::begin(&mut file, &dir);
        file.seek(SeekFrom::Start(4)).unwrap();
        let mut branch = handoff
            .finish(
                &mut file,
                &dir,
                ck_sim::Date {
                    year: 769,
                    month: 3,
                    day: 1,
                    hour: 0,
                },
            )
            .unwrap();
        assert_eq!(
            branch.months,
            vec![month(769, 1), month(769, 2), month(769, 3)]
        );
        branch.record(month_with_deaths(769, 4, 2));
        branch.record(month(769, 12));
        let future_b = Index::decode(&branch.trailer().unwrap()).unwrap();
        assert_eq!(future_a.years[0], future_b.years[0]);
        assert_ne!(future_a.years[1].1, future_b.years[1].1);
        assert_eq!(
            read_year(&dir, 42, 769, &future_a.years[1].1).unwrap()[3].places[0].pops[0].deaths,
            1
        );
        assert_eq!(
            read_year(&dir, 42, 769, &future_b.years[1].1).unwrap()[3].places[0].pops[0].deaths,
            2
        );
        drop(branch);
        drop(original);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// Read a listed immutable year only if its bytes match its recorded hash.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "The viewer in the next history slice consumes stored years."
    )
)]
pub(super) fn read_year(
    saves: &std::path::Path,
    campaign: u64,
    year: u16,
    hash: &[u8; 32],
) -> Option<Vec<MonthRecord>> {
    let hex = hash_hex(hash);
    let path = saves
        .join("history")
        .join(format!("{campaign:016x}"))
        .join(format!("{year:04}-{hex}.ckhist"));
    let bytes = std::fs::read(path).ok()?;
    if <[u8; 32]>::from(Sha256::digest(&bytes)) != *hash {
        return None;
    }
    let months = decode_year(&bytes)?;
    if months
        .iter()
        .any(|month| month.year != year || !(1..=12).contains(&month.month))
    {
        return None;
    }
    Some(months)
}

pub(super) enum Handoff {
    NoPrefix,
    Locked(u64, CampaignLock),
    Detached,
}

#[cfg(test)]
thread_local! {
    pub(super) static BEFORE_HANDOFF_FINISH: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::new(None);
}

impl Handoff {
    pub(super) fn begin(file: &mut std::fs::File, saves: &std::path::Path) -> Self {
        match campaign_prefix(file) {
            Ok(Some(campaign)) => match CampaignLock::shared(saves, campaign) {
                Ok(lock) => Self::Locked(campaign, lock),
                Err(_) => Self::Detached,
            },
            _ => Self::NoPrefix,
        }
    }

    /// `file` is positioned at END by the core reader. The candidate is installed only after core validation succeeds.
    pub(super) fn finish(
        self,
        file: &mut std::fs::File,
        saves: &std::path::Path,
        date: ck_sim::Date,
    ) -> std::io::Result<Recorder> {
        use std::io::Read;
        #[cfg(test)]
        BEFORE_HANDOFF_FINISH.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
        let mut detached = false;
        if let Self::Locked(campaign, lock) = self {
            let mut bytes = Vec::new();
            if file.read_to_end(&mut bytes).is_ok()
                && let Some(index) = Index::decode(&bytes)
                && index.campaign == campaign
                && let Some(months) = decode_year(&index.unfinished)
            {
                return Ok(Recorder {
                    index,
                    months,
                    lock: Some(lock),
                    saves: saves.to_owned(),
                    pending: Vec::new(),
                    start: std::time::SystemTime::now(),
                });
            }
        } else if matches!(self, Self::Detached) {
            detached = true;
        }
        let campaign = super::setup::operating_system_seed().map_err(std::io::Error::other)?;
        if detached {
            Ok(Recorder {
                index: Index {
                    campaign,
                    origin: (date.year, date.month),
                    years: Vec::new(),
                    unfinished: Vec::new(),
                },
                months: Vec::new(),
                lock: None,
                saves: saves.to_owned(),
                pending: Vec::new(),
                start: std::time::SystemTime::now(),
            })
        } else {
            Ok(Recorder::new(saves, campaign, (date.year, date.month)))
        }
    }
}

pub(super) fn save_references(saves: &std::path::Path) -> Option<Vec<Index>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut indices = Vec::new();
    for entry in std::fs::read_dir(saves).ok()? {
        let entry = entry.ok()?;
        if entry.path().extension() != Some(std::ffi::OsStr::new("cksave"))
            || !entry.file_type().ok()?.is_file()
        {
            continue;
        }
        let mut file = super::saves::open_regular(&entry.path()).ok()?;
        let header = ck_sim::SaveHeader::read_from(&mut file).ok()?;
        if header.format? < 4 {
            continue;
        }
        let len = file.metadata().ok()?.len();
        if len < 16 {
            return None;
        }
        file.seek(SeekFrom::End(-16)).ok()?;
        let mut footer = [0; 16];
        file.read_exact(&mut footer).ok()?;
        if &footer[8..] != FOOTER {
            return None;
        }
        let size = u64::from_le_bytes(footer[..8].try_into().ok()?);
        let offset = len.checked_sub(size.checked_add(16)?)?;
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut trailer = vec![0; usize::try_from(size.checked_add(16)?).ok()?];
        file.read_exact(&mut trailer).ok()?;
        let index = Index::decode(&trailer)?;
        decode_year(&index.unfinished)?;
        indices.push(index);
    }
    Some(indices)
}

fn collect(saves: &std::path::Path, start: std::time::SystemTime) -> Option<()> {
    let root = saves.join("history");
    std::fs::read_dir(root.join("locks")).ok()?;
    let folders = std::fs::read_dir(&root)
        .ok()?
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    save_references(saves)?;
    for folder in folders {
        let name = folder.file_name().into_string().ok()?;
        if name.len() != 16
            || !name
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            || !folder.file_type().ok()?.is_dir()
        {
            continue;
        }
        let campaign = u64::from_str_radix(&name, 16).ok()?;
        let Ok(_lock) = CampaignLock::exclusive(saves, campaign) else {
            continue;
        };
        #[cfg(test)]
        COLLECTION_LOCKED.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
        let references = save_references(saves)?;
        let keep: std::collections::BTreeSet<String> = references
            .iter()
            .filter(|index| index.campaign == campaign)
            .flat_map(|index| index.years.iter())
            .map(|(year, hash)| {
                let hex = hash_hex(hash);
                format!("{year:04}-{hex}.ckhist")
            })
            .collect();
        let folder_path = folder.path();
        for entry in std::fs::read_dir(&folder_path).ok()? {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let old_temporary = name.contains(".ckhist.tmp-")
                && entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .is_some_and(|time| time < start);
            if entry.file_type().ok()?.is_file()
                && ((name.ends_with(".ckhist") && !keep.contains(&name)) || old_temporary)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        if !references.iter().any(|index| index.campaign == campaign) {
            let _ = std::fs::remove_dir(&folder_path);
        }
    }
    Some(())
}

#[cfg(test)]
thread_local! {
    static COLLECTION_LOCKED: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::new(None);
}

#[cfg(test)]
mod collection_tests {
    use super::*;
    fn save(dir: &std::path::Path, format: u8, trailer: &[u8]) {
        let header = format!("format = {format}\n");
        let mut bytes = b"CKSAVE\0\x01".to_vec();
        bytes.extend_from_slice(&u32::try_from(header.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(trailer);
        std::fs::write(dir.join("one.cksave"), bytes).unwrap();
    }
    #[test]
    fn collection_keeps_references_and_stops_on_unknown_history() {
        use std::os::unix::fs::MetadataExt;
        let dir = std::env::temp_dir().join(format!("ck-history-collect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let game = Recorder::new(&dir, 42, (768, 10));
        drop(game);
        let folder = dir.join("history/000000000000002a");
        std::fs::create_dir_all(&folder).unwrap();
        let keep = format!("0768-{}.ckhist", "07".repeat(32));
        let remove = format!("0769-{}.ckhist", "08".repeat(32));
        std::fs::write(folder.join(&keep), b"kept").unwrap();
        std::fs::write(folder.join(&remove), b"removed").unwrap();
        let lock_path = dir.join("history/locks/000000000000002a.lock");
        let inode = std::fs::metadata(&lock_path).unwrap().ino();
        let index = Index {
            campaign: 42,
            origin: (768, 10),
            years: vec![(768, [7; 32])],
            unfinished: encode_year(&[]).unwrap(),
        };
        save(&dir, 4, &index.encode().unwrap());
        let running = CampaignLock::shared(&dir, 42).unwrap();
        collect(&dir, std::time::SystemTime::now()).unwrap();
        assert!(
            folder.join(&remove).exists(),
            "a running game prevents collection"
        );
        drop(running);
        collect(&dir, std::time::SystemTime::now()).unwrap();
        assert!(folder.join(&keep).exists());
        assert!(!folder.join(&remove).exists());
        std::fs::write(folder.join(&remove), b"still here").unwrap();
        save(&dir, 4, b"damaged trailer");
        assert!(collect(&dir, std::time::SystemTime::now()).is_none());
        assert!(folder.join(&remove).exists());
        let mut damaged_index = index.clone();
        damaged_index.unfinished = b"bad year".to_vec();
        save(&dir, 4, &damaged_index.encode().unwrap());
        assert!(collect(&dir, std::time::SystemTime::now()).is_none());
        assert!(folder.join(&remove).exists());
        save(&dir, 3, b"");
        collect(&dir, std::time::SystemTime::now()).unwrap();
        assert!(!folder.exists());
        assert_eq!(std::fs::metadata(&lock_path).unwrap().ino(), inode);
        let recreated = CampaignLock::shared(&dir, 42).unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        assert_eq!(std::fs::metadata(&lock_path).unwrap().ino(), inode);
        drop(recreated);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

pub(super) fn hash_hex(hash: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in hash {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 15)]));
    }
    out
}

#[cfg(test)]
mod process_tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

    #[test]
    fn lock_child() {
        let Some(dir) = std::env::var_os("CK_HISTORY_CHILD_DIR") else {
            return;
        };
        if let Some(initial) = std::env::var_os("CK_HISTORY_SWAP_CHILD") {
            use std::io::{Seek, SeekFrom};
            let dir = std::path::Path::new(&dir);
            let initial: u64 = initial.to_string_lossy().parse().unwrap();
            let target = if initial == 42 { 43 } else { 42 };
            let old = Recorder::new(dir, initial, (768, 12));
            assert!(old.lock.is_some());
            println!("HISTORY_READY");
            std::io::stdout().flush().unwrap();
            let mut byte = [0];
            std::io::stdin().read_exact(&mut byte).unwrap();
            let path = dir.join(format!("{target}.cksave"));
            let bytes = std::fs::read(&path).unwrap();
            let offset = bytes
                .windows(MAGIC.len())
                .position(|part| part == MAGIC)
                .unwrap();
            let mut file = std::fs::File::open(path).unwrap();
            let handoff = Handoff::begin(&mut file, dir);
            file.seek(SeekFrom::Start(offset as u64)).unwrap();
            let mut new = handoff
                .finish(
                    &mut file,
                    dir,
                    ck_sim::Date {
                        year: 768,
                        month: 12,
                        day: 1,
                        hour: 0,
                    },
                )
                .unwrap();
            drop(old);
            assert_eq!(
                Index::decode(&new.trailer().unwrap()).unwrap().campaign,
                target
            );
            println!("HISTORY_SWAPPED");
            std::io::stdout().flush().unwrap();
            std::io::stdin().read_exact(&mut byte).unwrap();
            return;
        }
        if std::env::var_os("CK_HISTORY_LOAD_CHILD").is_some() {
            use std::io::{Seek, SeekFrom};
            let dir = std::path::Path::new(&dir);
            let probe = CampaignLock::open(dir, 42).unwrap();
            assert!(probe.try_lock_shared().is_err());
            drop(probe);
            println!("HISTORY_WAITING");
            std::io::stdout().flush().unwrap();
            let path = dir.join("one.cksave");
            let bytes = std::fs::read(&path).unwrap();
            let offset = bytes
                .windows(MAGIC.len())
                .position(|part| part == MAGIC)
                .unwrap();
            let mut file = std::fs::File::open(path).unwrap();
            let handoff = Handoff::begin(&mut file, dir);
            file.seek(SeekFrom::Start(offset as u64)).unwrap();
            let mut recorder = handoff
                .finish(
                    &mut file,
                    dir,
                    ck_sim::Date {
                        year: 768,
                        month: 10,
                        day: 1,
                        hour: 0,
                    },
                )
                .unwrap();
            let index = Index::decode(&recorder.trailer().unwrap()).unwrap();
            assert_eq!(index.campaign, 42);
            assert_eq!(index.years.len(), 1);
            assert!(read_year(dir, 42, index.years[0].0, &index.years[0].1).is_some());
            println!("HISTORY_LOADED");
            std::io::stdout().flush().unwrap();
            let mut byte = [0];
            std::io::stdin().read_exact(&mut byte).unwrap();
            return;
        }
        println!("HISTORY_WAITING");
        std::io::stdout().flush().unwrap();
        let _lock = CampaignLock::shared(std::path::Path::new(&dir), 42).unwrap();
        println!("HISTORY_LOCKED");
        std::io::stdout().flush().unwrap();
        let mut byte = [0];
        std::io::stdin().read_exact(&mut byte).unwrap();
    }

    fn child(dir: &std::path::Path) -> (Child, ChildStdin, BufReader<ChildStdout>) {
        child_mode(dir, false)
    }

    fn child_mode(
        dir: &std::path::Path,
        load: bool,
    ) -> (Child, ChildStdin, BufReader<ChildStdout>) {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "game::history::process_tests::lock_child",
                "--nocapture",
            ])
            .env("CK_HISTORY_CHILD_DIR", dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        if load {
            command.env("CK_HISTORY_LOAD_CHILD", "1");
        }
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        (child, stdin, stdout)
    }

    fn swap_child(
        dir: &std::path::Path,
        initial: u64,
    ) -> (Child, ChildStdin, BufReader<ChildStdout>) {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "game::history::process_tests::lock_child",
                "--nocapture",
            ])
            .env("CK_HISTORY_CHILD_DIR", dir)
            .env("CK_HISTORY_SWAP_CHILD", initial.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        (child, stdin, stdout)
    }

    #[test]
    fn games_swap_campaigns_across_processes_and_collectors_skip_new_locks() {
        let dir = crate::testing::Dir::new();
        for campaign in [42, 43] {
            let mut recorder = Recorder::new(&dir.0, campaign, (768, 12));
            recorder.record(MonthRecord {
                year: 768,
                month: 12,
                places: vec![],
            });
            let trailer = recorder.trailer().unwrap();
            drop(recorder);
            let header = b"format = 4\n";
            let mut bytes = b"CKSAVE\0\x01".to_vec();
            bytes.extend_from_slice(&u32::try_from(header.len()).unwrap().to_le_bytes());
            bytes.extend_from_slice(header);
            bytes.extend_from_slice(&trailer);
            std::fs::write(dir.0.join(format!("{campaign}.cksave")), bytes).unwrap();
            let folder = dir.0.join("history").join(format!("{campaign:016x}"));
            std::fs::write(folder.join("orphan.ckhist"), b"orphan").unwrap();
        }
        let (mut a, mut a_in, mut a_out) = swap_child(&dir.0, 42);
        let (mut b, mut b_in, mut b_out) = swap_child(&dir.0, 43);
        until(&mut a_out, "HISTORY_READY");
        until(&mut b_out, "HISTORY_READY");
        a_in.write_all(b"x").unwrap();
        b_in.write_all(b"x").unwrap();
        until(&mut a_out, "HISTORY_SWAPPED");
        until(&mut b_out, "HISTORY_SWAPPED");
        collect(&dir.0, std::time::SystemTime::now()).unwrap();
        let orphan = |campaign| {
            dir.0
                .join("history")
                .join(format!("{campaign:016x}"))
                .join("orphan.ckhist")
        };
        assert!(orphan(42).exists() && orphan(43).exists());
        a_in.write_all(b"x").unwrap();
        assert!(a.wait().unwrap().success());
        collect(&dir.0, std::time::SystemTime::now()).unwrap();
        assert!(orphan(42).exists());
        assert!(!orphan(43).exists());
        b_in.write_all(b"x").unwrap();
        assert!(b.wait().unwrap().success());
    }

    #[test]
    fn loading_waits_for_real_collection_then_finds_listed_year() {
        let dir = crate::testing::Dir::new();
        let mut recorder = Recorder::new(&dir.0, 42, (768, 12));
        recorder.record(MonthRecord {
            year: 768,
            month: 12,
            places: vec![],
        });
        let trailer = recorder.trailer().unwrap();
        drop(recorder);
        let header = b"format = 4\n";
        let mut bytes = b"CKSAVE\0\x01".to_vec();
        bytes.extend_from_slice(&u32::try_from(header.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(header);
        bytes.extend_from_slice(&trailer);
        std::fs::write(dir.0.join("one.cksave"), bytes).unwrap();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let root = dir.0.clone();
        let collector = std::thread::spawn(move || {
            COLLECTION_LOCKED.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    locked_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }));
            });
            collect(&root, std::time::SystemTime::now())
        });
        locked_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let (mut process, mut stdin, mut stdout) = child_mode(&dir.0, true);
        until(&mut stdout, "HISTORY_WAITING");
        release_tx.send(()).unwrap();
        assert!(collector.join().unwrap().is_some());
        until(&mut stdout, "HISTORY_LOADED");
        assert!(CampaignLock::exclusive(&dir.0, 42).is_err());
        stdin.write_all(b"x").unwrap();
        assert!(process.wait().unwrap().success());
    }

    fn until(output: &mut BufReader<ChildStdout>, marker: &str) {
        loop {
            let mut line = String::new();
            assert_ne!(
                output.read_line(&mut line).unwrap(),
                0,
                "child exited before {marker}"
            );
            if line.contains(marker) {
                break;
            }
        }
    }

    #[test]
    fn locks_coordinate_running_games_and_collectors_across_processes() {
        let dir = std::env::temp_dir().join(format!("ck-history-process-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = CampaignLock::shared(&dir, 42).unwrap();
        let (mut process, mut stdin, mut stdout) = child(&dir);
        until(&mut stdout, "HISTORY_WAITING");
        until(&mut stdout, "HISTORY_LOCKED");
        assert!(CampaignLock::exclusive(&dir, 42).is_err());
        drop(first);
        assert!(CampaignLock::exclusive(&dir, 42).is_err());
        stdin.write_all(b"x").unwrap();
        assert!(process.wait().unwrap().success());
        let collector = CampaignLock::exclusive(&dir, 42).unwrap();
        let (mut process, mut stdin, mut stdout) = child(&dir);
        until(&mut stdout, "HISTORY_WAITING");
        drop(collector);
        until(&mut stdout, "HISTORY_LOCKED");
        stdin.write_all(b"x").unwrap();
        assert!(process.wait().unwrap().success());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
