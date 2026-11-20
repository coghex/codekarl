//! The name of a child at creation: a relative's name by chance, else the culture's pool (CK-365, characters slice 5b).
//!
//! A child (never a founder) created without a name first tries the kinds of namesake of `names.toml` in the data's order. The
//! candidates are real characters of the child's side (a son's father's side, a daughter's mother's side), live or dead as each
//! kind says. A kind with no candidate passes its chance on, **added to the next kind's base**; for a kind with candidates the
//! chance is `min(1, (base + carried) x (1 + the highest importance among them))`, rolled on purpose `namesake.<kind>`, and on a
//! success one candidate is picked weighted `1 + importance` on purpose `namesake.pick`. The base names of the child's living full
//! siblings and of the earlier children of the same delivery are excluded: from the candidates and from the pool draw. Everything is
//! exact (parts per billion and thousandths); no floating point.
#![deny(clippy::float_arithmetic)]

use crate::{
    ArchiveSink, Character, CharacterStore, Context, Error, Office, Sex, UNKNOWN,
    genetics::{below, roll, site},
};
use ck_economy::genetics::PPB;
use ck_sim::GameTime;
use std::collections::{BTreeMap, BTreeSet};

/// One in thousandths: the importance factor of a candidate is `1000 + importance`.
const ONE: u128 = 1000;

/// What the name of a child is decided by, besides the pool.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    /// The name of the relative the child is named for, when the namesake roll succeeded.
    pub namesake: Option<String>,
    /// The names the pool draw must avoid: the living full siblings' and the earlier children of the delivery.
    pub excluded: BTreeSet<String>,
}

/// A way to read a record that is not live: the archive.
pub(crate) type Archived<'a> = &'a dyn ArchiveSink;

/// The importance of a candidate in thousandths: 1 for a ruler (the marker of its record, live or archived), else 0. `heir` and
/// `high_cleric` count 0 here (5c adds a ruling house).
fn importance_milli(c: &Character) -> u128 {
    if c.office == Some(Office::Ruler) {
        ONE
    } else {
        0
    }
}

/// Whether `c` was dead at `birth`: its stored time of death is not after the stored time of the birth. A death of the same tick
/// counts (the scheduler runs the deaths before the deliveries of a tick); one later on the same day does not.
fn died_by(c: &Character, birth: GameTime) -> bool {
    c.death.is_some_and(|died| died <= birth)
}

/// A reader of live-then-archived records.
struct Records<'a> {
    store: &'a CharacterStore,
    archive: Archived<'a>,
}

impl Records<'_> {
    /// The record of `id`: `None` for the unknown character and for one nobody holds.
    fn get(&self, id: u64) -> Result<Option<Character>, Error> {
        if id == UNKNOWN {
            return Ok(None);
        }
        if let Some(live) = self.store.live.get(&id) {
            return Ok(Some(live.clone()));
        }
        self.archive.get(id).map_err(|_| Error::Archive)
    }
}

/// The candidates of each kind for a child of `sex` born `birth`, from `side` (the father for a son, the mother for a daughter):
///
/// - `father`: the parent of the side, only if dead before the birth;
/// - `grandfather`: that parent's parent of the same sex (father's father, mother's mother), if known, live or dead;
/// - `uncle`: a brother of the father (sister of the mother) who has died: an archived child (by recorded parentage) of either of
///   the parent's own parents;
/// - `fathers_friend`: none until slice 6;
/// - `ancestor`: any known dead ancestor of the child's sex on that side beyond the grandfather's generation.
fn candidates(
    records: &Records<'_>,
    kind: &str,
    sex: Sex,
    birth: GameTime,
    side: &Character,
) -> Result<Vec<Character>, Error> {
    let same_line = |c: &Character| if sex == Sex::Male { c.father } else { c.mother };
    let mut found: BTreeMap<u64, Character> = BTreeMap::new();
    match kind {
        "father" => {
            if died_by(side, birth) {
                found.insert(side.id, side.clone());
            }
        }
        "grandfather" => {
            if let Some(grandparent) = records.get(same_line(side))? {
                found.insert(grandparent.id, grandparent);
            }
        }
        "uncle" => {
            // Siblings by recorded parentage, not by a parent's list of children: a child born after a parent was archived is linked
            // from the child only. The dead are archived, so the archive's index of children by parent finds them.
            let mut siblings = BTreeSet::new();
            for parent in [side.father, side.mother] {
                if parent != UNKNOWN {
                    siblings.extend(
                        records
                            .archive
                            .children_of(parent)
                            .map_err(|_| Error::Archive)?,
                    );
                }
            }
            siblings.remove(&side.id);
            for id in siblings {
                if let Some(sibling) = records.get(id)?
                    && sibling.sex == sex
                    && died_by(&sibling, birth)
                {
                    found.insert(id, sibling);
                }
            }
        }
        "ancestor" => {
            // Generation 1 is the parent of the side; the grandparents are generation 2 and the candidates start at 3.
            let mut seen = BTreeSet::from([side.id]);
            let mut generation = vec![side.clone()];
            let mut depth = 1_u32;
            while !generation.is_empty() {
                let mut next = Vec::new();
                for c in &generation {
                    for parent in [c.father, c.mother] {
                        if parent != UNKNOWN
                            && seen.insert(parent)
                            && let Some(record) = records.get(parent)?
                        {
                            next.push(record);
                        }
                    }
                }
                depth += 1;
                if depth >= 3 {
                    for record in &next {
                        if record.sex == sex && died_by(record, birth) {
                            found.insert(record.id, record.clone());
                        }
                    }
                }
                generation = next;
            }
        }
        // `fathers_friend` has no candidate until a friend strand exists (slice 6).
        _ => {}
    }
    Ok(found.into_values().collect())
}

/// Decide the name of a child about to be created: the names to avoid and, by the namesake rules, the relative it may be named for.
///
/// `parents` are the father and the mother (`None` for an unknown one); `earlier` are the names already chosen for earlier children
/// of the same delivery.
///
/// # Errors
/// An archive read failure, or a roll that does not fit.
#[expect(
    clippy::too_many_arguments,
    reason = "the child's id, sex and birth, its parents, the store, the archive and the delivery's earlier names are all inputs"
)]
pub(crate) fn plan(
    ctx: &Context<'_>,
    store: &CharacterStore,
    archive: Archived<'_>,
    id: u64,
    sex: Sex,
    birth: GameTime,
    parents: [Option<&Character>; 2],
    earlier: &BTreeSet<String>,
) -> Result<Plan, Error> {
    let [father, mother] = parents;
    let mut excluded = earlier.clone();
    // The base names of the living full siblings: both parents known and the same.
    if let (Some(father), Some(mother)) = (father, mother) {
        excluded.extend(
            store
                .live
                .values()
                .filter(|c| c.father == father.id && c.mother == mother.id)
                .map(|c| c.name.clone()),
        );
    }
    let side = if sex == Sex::Male { father } else { mother };
    let Some(side) = side else {
        return Ok(Plan {
            namesake: None,
            excluded,
        });
    };
    let records = Records { store, archive };
    let mut carried: u128 = 0;
    for (kind, base) in &ctx.lifecycle.namesakes.order {
        let mut found = candidates(&records, kind, sex, birth, side)?;
        found.retain(|c| !excluded.contains(&c.name));
        let Some(highest) = found.iter().map(importance_milli).max() else {
            // No candidate: the chance passes on, added to the next kind's base.
            carried += u128::from(*base);
            continue;
        };
        let chance = ((u128::from(*base) + carried) * (ONE + highest) / ONE).min(u128::from(PPB));
        carried = 0;
        let hit = roll(
            &mut site(ctx, id, "", &format!("namesake.{kind}")),
            u32::try_from(chance).map_err(|_| Error::Lifecycle)?,
        )?;
        if !hit {
            continue;
        }
        let weights: Vec<u128> = found.iter().map(|c| ONE + importance_milli(c)).collect();
        let total: u128 = weights.iter().sum();
        let mut pick = u128::from(below(
            &mut site(ctx, id, "", "namesake.pick"),
            u64::try_from(total).map_err(|_| Error::Lifecycle)?,
        )?);
        for (candidate, weight) in found.iter().zip(&weights) {
            if pick < *weight {
                return Ok(Plan {
                    namesake: Some(candidate.name.clone()),
                    excluded,
                });
            }
            pick -= weight;
        }
    }
    Ok(Plan {
        namesake: None,
        excluded,
    })
}
