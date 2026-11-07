# Ireland in 768: sources, rules and guesswork

This scenario is Ireland as it stood on 9 October 768, for milestone 7 (CK-194). It holds the skeleton of attested places: the great
church settlements, the royal seats and the lesser seats where a site is attested. There are no farms or villages, and nothing invented.
Every place was placed by hand at its named site; none was placed by noise or procedure.

| File | What it is |
|---|---|
| `scenario.toml` | The id (`ireland_768`), the map (`ireland`, resolved to `assets/map/ireland/`), the start date and the locations file |
| `locations.toml` | One `[[location]]` per place |
| `SOURCES.md` | This file |

The kinds, their sizes, catchment radii and glyphs are `assets/map/ireland/location_kinds.toml`, shared by every scenario on that map.

**94 places:** 74 monasteries, 8 royal sites, 10 ringforts and 2 crannogs. Confidence: 37 high, 47 medium and 10 low.

## The owner's decisions

Quoted verbatim, with their dates (relayed by ck-manager, whose message ids are given).

- **2026-11-05** (CK-181): "noise shouldnt be used to place these".
- **2026-11-06** (`paawiu7vyeab88r4dqiure3hca`), the three questions of the slice 6 session:
  1. names: "modern Irish, English form in the guesswork log";
  2. density: "about 100 (attested skeleton, nothing invented)";
  3. kinds and buildings: "accept as listed (monastery, royal_site, ringfort, crannog; church, high_cross, enclosure; no round towers)".
- **2026-11-06** (`g3wpwaetwu7maemckrwrtycjiw`), on the island monasteries of Iniscealtra (Lough Derg) and Inisfallen (Lough Leane):
  "Make the islands land". The map's edits r8 does this; see `assets/map/ireland/SOURCES.md`.
- **2026-11-06** (`w9wu4q8trbn96eisf5vy5eh9je`), sign-off on the preview of draft 3: "the places look good, i approve".

## The rule for each kind

- **`monastery`:** a great church settlement with good annalistic attestation before 768. Size ranks its prominence in the 8th-century
  annals, not its population, which is unknown. Size 4 is kept for Armagh, Clonmacnoise and Kildare.
- **`royal_site`:** the seat of a provincial or over-king, where the seat is known.
- **`ringfort`:** a lesser king's or lord's seat, attested in the annals or by excavation with 8th-century occupation.
- **`crannog`:** a lake-dwelling seat, attested as above.
- **`longphort`:** defined but unused, because the Norse arrive in 795 and found Dublin about 841.
- **Buildings:** `church` at every monastery and at royal seats with a church; `enclosure` (the rath, cashel or palisade) at royal seats,
  ringforts, crannogs and walled church sites; `high_cross` only where an 8th-century date is argued. No round towers: they are later,
  from about 900.

| Kind | Sizes | Default `catchment_m` by size | Buildings in this draft |
|---|---|---|---|
| `monastery` | 1–4 | 3,000 / 5,000 / 7,000 / 9,000 | `church` at every one; `enclosure` at the four largest and the walled island sites; `high_cross` at three (G6) |
| `royal_site` | 1–3 | 4,000 / 6,000 / 8,000 | `enclosure`; `church` also at Downpatrick and Clogher (G4) |
| `ringfort` | 1–2 | 2,500 / 3,500 | `enclosure` |
| `crannog` | 1–2 | 2,500 / 3,500 | `enclosure` (the palisade) |
| `longphort` | 1–3 | 3,000 / 5,000 / 7,000 | defined but unused: the Norse arrive in 795 |

## Sources

- **Monasteries:** Gwynn & Hadcock, *Medieval Religious Houses: Ireland* (1970), and the annals (AU, AFM).
- **Royal sites and ringforts:** Byrne, *Irish Kings and High-Kings* (1973), and the annals.
- **Excavated sites:** Hencken for Cahercommaun (1938) and Lagore (1950); Ó Ríordáin for Carraig Aille (1949); Eogan for Knowth; Lynn
  for Rathmore; Bradley for Moynagh Lough (excavations.ie).
- **Positions** are each named site's, to about 100 m. Ten low-confidence sites were checked against public records (the Irish Stones
  survey, excavations.ie, the RSAI, heritage listings) and nine were corrected.
- **What was not checked:** the print works above were not checked online; their attributions rest on the designer's knowledge of them.
  The modern Irish names were not all checked against logainm.ie and Placenames NI. Later research should correct either, through the
  guesswork log.
- No OpenStreetMap geometry is used, for licence reasons (as CK-181).

## The places

The `name` is modern Irish; the English form is given here.

| id | Name | English | Kind | Size | Confidence | Source |
|---|---|---|---|---|---|---|
| `armagh` | Ard Mhacha | Armagh | monastery | 4 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `clonmacnoise` | Cluain Mhic Nóis | Clonmacnoise | monastery | 4 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `kildare` | Cill Dara | Kildare | monastery | 4 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `glendalough` | Gleann Dá Loch | Glendalough | monastery | 3 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `bangor` | Beannchar | Bangor | monastery | 3 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `durrow` | Darú | Durrow | monastery | 3 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `clonard` | Cluain Ioraird | Clonard | monastery | 3 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `lismore` | Lios Mór | Lismore | monastery | 3 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `clonfert` | Cluain Fearta | Clonfert | monastery | 3 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `iniscealtra` | Inis Cealtra | Iniscealtra | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `emly` | Imleach Iúir | Emly | monastery | 3 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `cork` | Corcaigh | Cork | monastery | 3 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `movilla` | Maigh Bhile | Movilla | monastery | 3 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `derry` | Doire | Derry | monastery | 2 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `inishmurray` | Inis Muirígh | Inishmurray | monastery | 1 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `skellig_michael` | Sceilg Mhichíl | Skellig Michael | monastery | 1 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `monasterboice` | Mainistir Bhuithe | Monasterboice | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `finglas` | Fionnghlas | Finglas | monastery | 2 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `lusk` | Lusca | Lusk | monastery | 2 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `swords` | Sord | Swords | monastery | 2 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `terryglass` | Tír Dhá Ghlas | Terryglass | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `birr` | Biorra | Birr | monastery | 2 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `seirkieran` | Saighir Chiaráin | Seirkieran | monastery | 2 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `roscrea` | Ros Cré | Roscrea | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `lorrha` | Lothra | Lorrha | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `mungret` | Mungairit | Mungret | monastery | 2 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `ardfert` | Ard Fhearta | Ardfert | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `inisfallen` | Inis Faithlinn | Inisfallen | monastery | 1 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `cloyne` | Cluain | Cloyne | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `ardmore` | Aird Mhór | Ardmore | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `ferns` | Fearna | Ferns | monastery | 3 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `taghmon` | Teach Munna | Taghmon | monastery | 1 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `aghaboe` | Achadh Bhó | Aghaboe | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `old_leighlin` | Seanleithghlinn | Old Leighlin | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `clonenagh` | Cluain Eidhneach | Clonenagh | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `slane` | Baile Shláine | Slane | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `duleek` | Damhliag | Duleek | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `trim` | Baile Átha Troim | Trim | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `louth` | Lú | Louth | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `dromiskin` | Droim Ineasclainn | Dromiskin | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `dunleer` | Lann Léire | Dunleer | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `fore` | Baile Fhobhair | Fore | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `clones` | Cluain Eois | Clones | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `devenish` | Daimhinis | Devenish | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `ardstraw` | Ard Sratha | Ardstraw | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `raphoe` | Ráth Bhoth | Raphoe | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `fahan` | Fathain | Fahan | monastery | 1 | low | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `carndonagh` | Carn Domhnach | Carndonagh | monastery | 1 | low | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `drumcliff` | Droim Chliabh | Drumcliff | monastery | 2 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `cong` | Conga | Cong | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `mayo` | Maigh Eo | Mayo | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `tuam` | Tuaim | Tuam | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `annaghdown` | Eanach Dhúin | Annaghdown | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `kilmacduagh` | Cill Mhic Duach | Kilmacduagh | monastery | 1 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `killeany` | Cill Éinne | Killeany | monastery | 2 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `inishbofin` | Inis Bó Finne | Inishbofin | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `tory` | Toraigh | Tory | monastery | 1 | low | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `roscommon` | Ros Comáin | Roscommon | monastery | 2 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `elphin` | Ail Finn | Elphin | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `ardagh` | Ardach | Ardagh | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `scattery` | Inis Cathaigh | Scattery Island | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `nendrum` | Naondroim | Nendrum | monastery | 2 | high | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `connor` | Coinnire | Connor | monastery | 1 | low | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `antrim` | Aontroim | Antrim | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `saul` | Sabhall | Saul | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `dromore` | Droim Mór | Dromore | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `clondalkin` | Cluain Dolcáin | Clondalkin | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `rahan` | Raithean | Rahan | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `killeigh` | Cill Aichidh | Killeigh | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `lemanaghan` | Liath Mancháin | Lemanaghan | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `killala` | Cill Ala | Killala | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `ross_carbery` | Ros Ó gCairbre | Rosscarbery | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `ahenny` | Áth Eine | Ahenny | monastery | 1 | low | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `st_mullins` | Tigh Moling | St Mullins | monastery | 1 | medium | Gwynn & Hadcock, Medieval Religious Houses: Ireland (1970); annals (AU, AFM) |
| `tara` | Teamhair | Tara | royal_site | 3 | high | Byrne, Irish Kings and High-Kings (1973); annals |
| `cashel` | Caiseal | Cashel | royal_site | 3 | high | Byrne, Irish Kings and High-Kings (1973); annals |
| `rathcroghan` | Ráth Cruachan | Rathcroghan | royal_site | 3 | medium | Byrne, Irish Kings and High-Kings (1973); annals |
| `ailech` | Grianán Ailigh | Grianán of Aileach | royal_site | 3 | medium | Byrne, Irish Kings and High-Kings (1973); annals |
| `naas` | An Nás | Naas | royal_site | 2 | medium | Byrne, Irish Kings and High-Kings (1973); annals |
| `downpatrick` | Dún Pádraig | Downpatrick | royal_site | 2 | medium | Byrne, Irish Kings and High-Kings (1973); annals |
| `clogher` | Clochar | Clogher | royal_site | 2 | medium | Byrne, Irish Kings and High-Kings (1973); annals |
| `uisneach` | Uisneach | Uisneach | royal_site | 2 | medium | Byrne, Irish Kings and High-Kings (1973); annals |
| `knowth` | Cnóbha | Knowth | ringfort | 2 | high | Byrne (1973); Eogan's Knowth excavations |
| `lyons_hill` | Liamhain | Lyons Hill | ringfort | 1 | medium | Byrne, Irish Kings and High-Kings (1973); annals |
| `rathangan` | Ráth Iomgháin | Rathangan | ringfort | 1 | medium | Byrne, Irish Kings and High-Kings (1973); annals |
| `dunamase` | Dún Másc | Dunamase | ringfort | 1 | medium | Byrne, Irish Kings and High-Kings (1973); annals |
| `knockainey` | Cnoc Áine | Knockainey | ringfort | 1 | medium | Byrne, Irish Kings and High-Kings (1973); annals |
| `bruree` | Brú Rí | Bruree | ringfort | 1 | medium | Byrne, Irish Kings and High-Kings (1973); annals |
| `rathmore` | An Ráth Mhór | Rathmore | ringfort | 1 | low | Byrne (1973); Lynn's Rathmore excavations |
| `dunseverick` | Dún Sobhairce | Dunseverick | ringfort | 1 | medium | Byrne, Irish Kings and High-Kings (1973); annals |
| `cahercommaun` | Cathair Chomáin | Cahercommaun | ringfort | 1 | low | Hencken, Cahercommaun (1938) |
| `carraig_aille` | Carraig Aille | Carraig Aille | ringfort | 1 | low | Ó Ríordáin's Lough Gur excavations (1949) |
| `lagore` | Loch Gabhair | Lagore | crannog | 2 | low | Hencken, Lagore crannog (1950); Byrne (1973) |
| `moynagh` | Maigh nEach | Moynagh Lough | crannog | 1 | low | Bradley's Moynagh Lough excavations (excavations.ie) |

## Guesswork log

Each row is a judgement that later research can correct. Each place's own note (why it is placed as it is) follows the log.

| # | Place or omission | Judgement and reason |
|---|---|---|
| G1 | Iniscealtra, Inisfallen | Island monasteries whose islands are below the map's 500 m resolution, so the derivation made their cells lake. The owner, 2026-11-06: "Make the islands land". Edits r8 paints the one cell containing each site pasture; each island is then one cell, about 25 ha (Holy Island is about 20 ha, Inisfallen about 9 ha). Devenish, Inishmurray, Skellig Michael, Scattery, Tory, Inishbofin, Killeany (Inis Mór) and Nendrum (Mahee) were already on land cells. Any other site found below the resolution follows spec r3's rule (the nearest land cell) unless the owner rules otherwise. |
| G2 | Kells, Tallaght, Castledermot | Not placed: founded after 768 (Kells about 807, Tallaght 769, Castledermot about 800). |
| G3 | Emain Macha, Dún Ailinne | Not placed: long abandoned by 768. |
| G4 | Downpatrick, Clogher, Ferns | Both royal seat and chief church. One cell holds one location, so Downpatrick and Clogher are `royal_site` with a `church`, and Ferns is a `monastery` (its church dominates). |
| G5 | Ailech, Uisneach | Ailech is placed at the Grianán, the traditional identification. Uisneach is placed as Clann Cholmáin's royal seat (Domnall Midi, d. 763), not as an assembly site; the owner chose no assembly kind. |
| G6 | High crosses at Ahenny, Carndonagh and Fahan | The dates of the crosses run from the 7th to the 9th century. They are placed where an 8th-century date is argued; Clonmacnoise, Monasterboice and Moone are later, so none there. |
| G7 | Cahercommaun, Carraig Aille, Moynagh Lough | Excavated high-status sites with 8th-century occupation, but no annal names whose seat they were. Included at low confidence as attested sites, not invented ones. |
| G8 | Lagore | Loch Gabhair was drained in the 19th century; the modern terrain is land. The crannog is placed on its land cell. |
| G9 | Regions with no seat | No royal seat placed for Osraige, Uí Maine, Cenél Conaill, the Déisi, Corcu Loígde, Ciarraige Luachra or Breifne: no site is attested well enough. Their great churches stand for them (for example Aghaboe and Seirkieran for Osraige). |
| G10 | Assembly sites, hermitages | Tailtiu and the like are not placed, and there is no hermitage kind; the owner accepted the four kinds as listed. |
| G11 | Dublin | No longphort: the Norse found Dublin about 841. The church sites of the Dublin area (Finglas, Swords, Lusk and Clondalkin) are placed. |
| G12 | Round towers | None placed: they are later, from about 900. |
| G13 | Name forms | Moynagh Lough has no logainm.ie entry; the crannog takes its townland's form, **Maigh nEach**. An Ráth Mhór (Rathmore, Grange of Nilteen, Co. Antrim) remains the designer's form: no Irish form was found online, and Placenames NI is still to be checked. |
| G14 | Sizes | Sizes rank prominence in the 8th-century annals, not population, which is unknown. Size 4 is for Armagh, Clonmacnoise and Kildare. |
| G15 | Lagore position | The two references disagree by about 1.5 km (see its note); the draft takes the excavation report's. The preview will show it against the terrain. |

### Notes by place

| id | Note |
|---|---|
| `armagh` | Primatial church of Pátraic's paruchia |
| `clonmacnoise` | Earliest crosses about 800 or later: no high_cross |
| `kildare` | Brigit's double house; Uí Dúnlainge church |
| `glendalough` | Coemgen's monastery; between the two lakes |
| `bangor` | Comgall's monastery; the Antiphonary of Bangor |
| `durrow` | Columban house; Book of Durrow |
| `clonard` | Finnian's school; site position near the village |
| `lismore` | Mochutu's monastery |
| `clonfert` | Brendan's monastery |
| `iniscealtra` | Holy Island, Lough Derg; made land by edits r8 (owner, 2026-11-06) |
| `emly` | Ailbe's church; chief church of Munster in 768 |
| `cork` | Finbarr's monastery, St Fin Barre's site |
| `movilla` | Finnian's monastery near Newtownards |
| `derry` | Columban house; exact site within the walled city |
| `inishmurray` | Molaise's cashel on the island |
| `skellig_michael` | Island hermit monastery |
| `monasterboice` | Buite's monastery; the crosses are 10th century |
| `finglas` | Céli Dé house; Dublach d. 796 |
| `lusk` | Mac Cuilinn's monastery |
| `swords` | Columban foundation |
| `terryglass` | Colum mac Crimthainn's monastery |
| `birr` | Brendan of Birr; Cáin Adomnáin promulgated nearby 697 |
| `seirkieran` | Ciarán of Saigir; Osraige church |
| `roscrea` | Crónán's monastery |
| `lorrha` | Ruadán's monastery; Stowe Missal |
| `mungret` | Nessán's monastery |
| `ardfert` | Brendan's foundation |
| `inisfallen` | Island in Lough Leane; made land by edits r8 (owner, 2026-11-06) |
| `cloyne` | Colmán mac Lénéni's church |
| `ardmore` | Déclán's church |
| `ferns` | Máedóc's church; Uí Chennselaig's chief church and seat |
| `taghmon` | Fintan Munnu's monastery |
| `aghaboe` | Cainnech's chief house; Fergal (Virgil of Salzburg) left about 742 |
| `old_leighlin` | Laisrén's monastery |
| `clonenagh` | Fintan's monastery; position checked (Clonenagh church and cross slabs, S 3895 9572) |
| `slane` | Erc's church on the Hill of Slane |
| `duleek` | Cianán's stone church |
| `trim` | Lommán's church |
| `louth` | Mochta's monastery |
| `dromiskin` | Early church; abbots in the annals |
| `dunleer` | Abbots in the annals from the 8th century |
| `fore` | Féichín's monastery |
| `clones` | Tigernach's monastery |
| `devenish` | Molaise's island, Lough Erne |
| `ardstraw` | Eogan's church |
| `raphoe` | Columban church |
| `fahan` | Fahan Mura cross-slab, dated 7th to 9th century: high_cross is a judgement; position checked (C 3444 2632) |
| `carndonagh` | Carndonagh cross, dated 7th to 9th century: high_cross is a judgement; position checked (C 4629 4500) |
| `drumcliff` | Columban monastery |
| `cong` | Féichín's foundation |
| `mayo` | Mag nEó na Saxan, the English monks' house; Gerald d. 732 |
| `tuam` | Iarlaith's church |
| `annaghdown` | Brendan's foundation on Lough Corrib |
| `kilmacduagh` | Colmán mac Duach's church |
| `killeany` | Énda's monastery on Inis Mór |
| `inishbofin` | Colmán's monastery from 668 |
| `tory` | Columban island monastery |
| `roscommon` | Comán's monastery |
| `elphin` | Patrician church |
| `ardagh` | Mel's church |
| `scattery` | Senán's island monastery |
| `nendrum` | Mahee Island, Strangford Lough; triple cashel |
| `connor` | Dál nAraidi church; site position uncertain |
| `antrim` | Early monastery, Steeple site |
| `saul` | Patrician church |
| `dromore` | Colmán's monastery |
| `clondalkin` | Crónán Mochua's monastery |
| `rahan` | Carthach's first monastery |
| `killeigh` | Sinchell's monastery |
| `lemanaghan` | Manchán d. 665 |
| `killala` | Patrician church |
| `ross_carbery` | Fachtna's school |
| `ahenny` | Kilclispeen crosses, dated late 8th century: high_cross is a judgement; position checked (S 4133 2922) |
| `st_mullins` | Moling d. 697 |
| `tara` | Kingship of Tara, held by the Uí Néill high-kings |
| `cashel` | Eóganacht kings of Munster; the Rock |
| `rathcroghan` | Cruachain, Uí Briúin kings of Connacht |
| `ailech` | Ailech of Cenél nEógain; the identification with the Grianán is traditional |
| `naas` | Uí Dúnlainge kings of Leinster; the North Moat |
| `downpatrick` | Dún Lethglaise, Dál Fiatach kings of Ulaid, with its church |
| `clogher` | Uí Chremthainn kings of the Airgíalla, with Mac Cairthinn's church |
| `uisneach` | Clann Cholmáin kings of Mide (Domnall Midi d. 763) |
| `knowth` | Kings of Northern Brega, in the great mound |
| `lyons_hill` | Uí Dúnchada of Leinster |
| `rathangan` | Uí Failge |
| `dunamase` | Loígis; Dún Masc plundered 845 |
| `knockainey` | Eóganacht Áine; position of Cnoc Áine checked (RSAI record) |
| `bruree` | Dún Eochair Maige, Uí Fidgenti |
| `rathmore` | Ráith Mór Maige Line, Dál nAraidi; name form and position to check |
| `dunseverick` | Dál Riata in Ireland |
| `cahercommaun` | Stone fort occupied about 800: a judgement for 768; position checked (R 2817 9652) |
| `carraig_aille` | Stone forts occupied 8th to 9th century; position checked (R 6529 4069) |
| `lagore` | Kings of Southern Brega; lake drained in the 19th century. Position estimated from an excavation report placing the crannog about 1 km north-east of 53.5119, -6.5306; a Wikipedia-derived point (53.5081, -6.5064) disagrees by about 1.5 km |
| `moynagh` | Crannog built in the first half of the 7th century, occupied to the end of the 8th (palisade felled 748); position from the excavation record; name is the townland's (Maigh nEach) |
