# Ireland in 768: sources, rules and guesswork

This scenario is Ireland as it stood on 9 October 768, for milestone 7 (CK-194). It holds the skeleton of attested places: the great
church settlements, the royal seats and the lesser seats where a site is attested. There are no farms or villages, and nothing invented.
Every place was placed by hand at its named site; none was placed by noise or procedure.

| File | What it is |
|---|---|
| `scenario.toml` | The id (`ireland_768`), the map (`ireland`, resolved to `assets/map/ireland/`), the start date and the locations file |
| `locations.toml` | One `[[location]]` per place |
| `economy.toml` | The deterministic opening estimate of each place's people, buildings and stores, and the noble houses |
| `ireland-768.toml` | The research record of the real figures (CK-370): names, bynames, languages, places, notes. Read with the file below |
| `ireland-768-load.toml` | The structure of the real figures (CK-370): houses, offices, parents and spouses, dates, the dead parents, the known strands and the slots |
| `houses.toml` | The display names of the houses and subhouses the load file uses, and the dynasty shown before a name (CK-375) |
| `SOURCES.md` | This file |

The kinds, their sizes, catchment radii and glyphs are `assets/map/ireland/location_kinds.toml`, shared by every scenario on that map.

**Current CK-358 scenario:** 123 inhabited locations and two abandoned ceremonial sites. The entries and their source notes are in `locations.toml`; the older 94-place inventory below records the earlier draft.

The authoring sources in `.pipeline/coordination/history-scratch/scenario/` are `locations-r21.toml` (SHA-256 `bdb45cd7d06ba6ac956225cd831eabac6acd0dac8229f8dbf4dae090668d973d`), `locations-changes-r3.md` (`d692806ec9faddd81a6d6b4e74aeb086e5cd8442eaf0d3111815f49426e4a281`), and `p2-economy-places-r1.toml` (`7fa590c19aaadbc0a16720b918e2aaeb6271d653f902f48d1fc00709a00ccfa3`). The Lóegairi stand-in seat moved to a free land cell; Loch Léin keeps its r21 anchor. Two abandoned sites were activated under the approved CK-358 dispositions. Cahercommaun and Dunamase are built later and excluded.

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

## Current location kinds

Kinds describe the physical place. `monastery` is a church settlement, `ringfort` a defended earthen enclosure, `crannog` a lake dwelling, `cashel` a stone fort, `hillfort` a hill enclosure, `ceremonial_ground` an assembly site, and `promontory_fort` a defended headland. `longphort` is defined but unused in 768. `royal_site` has been retired; seats are not stored as a kind. Sizes, catchments, house numbers and glyphs are in `assets/map/ireland/location_kinds.toml`.

Emain Macha and Dún Ailinne are abandoned: they have no people, working buildings or catchment. Cahercommaun and Dunamase were built later and are not loaded.

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

## The places in the earlier 94-place draft

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

## Economy starting-data guesses (CK-225)

The owner-signed per-place culture and peasant table is recorded in `.pipeline/coordination/m9-design/data/places-r2.md` (owner sign-off relayed in #codekarl
msgid `cqarvdb99b8twq87h8h243ybri`; signed copy SHA-256
`8e6683d25872305f4d275415a5c47ae08a674c8c8f57e98a48e86c7d37597e98`). The authored values live in
`assets/scenarios/ireland-768/economy.toml`.

| Guess | Authored choice | Status |
|---|---|---|
| `birr` regional culture | Mumu | Guessed for the Mumu–Mide frontier; owner-approved in the signed table. |
| `killeany` regional culture | Connachta | Guessed for Aran; owner-approved in the signed table. |
| `cahercommaun` regional culture | Connachta | Guessed for the Burren; owner-approved in the signed table. |
| `clonmacnoise` peasant cultures | 50% Mide and Brega, 50% Connachta | The border monastery's mixed share is a guess; owner-approved in the signed table. Monks and labourers use Mide and Brega. |
| `inisfallen` peasant population | 0, with farm level 0 | Island-house guess; owner-approved in the signed table. |
| Pagan pockets | `cahercommaun` 20%, `killala` 15%, `cong` 15%, `tory` 10% of peasants | Design-invented remote pockets and shares; owner-approved in the signed table. |

## Housing start fixes (CK-229)

| Place | Authored start | Rationale |
|---|---|---|
| `skellig_michael` | 10 monks, 10 labourers, no peasants; 2 holdings levels | The M9 housing design requires this population to fit the island's 31 basic spaces, including the storehouse. The counts are a gameplay fixture, not a new historical estimate. |
| `inishmurray` | 50 peasants, 10 monks, 10 labourers; 3 farm levels and 2 holdings levels | The M9 housing design requires the coastal farm and populations to fit its 82 basic spaces, including the storehouse. The exact population split and levels are gameplay fixture choices. |

Skellig Michael's peasant-less opening is supported by the sourced community of about twelve monks and an abbot. Inishmurray keeps
peasants. The signed table resolves the Osraige and Dál Riata culture mappings to Mumu and Ulaid respectively.

## Noble houses and the demesne (CK-359, characters P1)

The noble pop type and the demesne building enter the shared rules (`assets/game/economy.toml`); the 33 noble houses of the 768 start
enter this scenario's `economy.toml` as `[nobles.<place>]` tables. Both are copies of the designer's approved files, kept in
`.pipeline/coordination/history-scratch/`; the owner's words in `owner-direction-designer-rN.md` govern them.

| Copy | Source | SHA-256 of the source | Approval |
|---|---|---|---|
| `noble` pop type, `demesne` building, their diet, comfort and ticking profiles (`assets/game/economy.toml`) | `scenario/p1-noble-economy-r1.toml` | `027ba6911380d433996d2d3a3b61a3600099265f241e011d412a4b96c45f6ebc` | owner, r190 (the pop) and r191 (the demesne); sizes and rates from r188 |
| `[nobles.*]` (`economy.toml` of this scenario): 33 places, 1,180 nobles, 590 labourer positions | `scenario/p1-nobles-start-r1.toml` | `8c50f823b52946105a3e3c97ff1c21b8c011535c5505acb63e8a470b93611835` | presented with r189 and standing; `holdings_levels` renamed `demesne_levels` by the designer |
| The rent rule (code, not data: `crates/economy`) | `rent-r1.md` | `17167aa86a6649f670ae544299cff86261372fbbe7b84f02c7edfef1e4871d64` | owner, r189 |

- **One noble pop per seat**: 20, 40 or 80 people by the seat's size (size 1 to 3); a seat in a church settlement takes the court's own
  size, not the church's. `dunseverick` is the one house whose culture (`dal_riatan`) is not its place's (`ulaid`): the Dál Riata
  stand-in seat.
- **The demesne**: one level per 20 nobles, worked by ten paid labourers a level. The shared rules name its owner pop type
  (`owner_kind = "noble"`) and keep the designer's "one noble covers 10 labourers" as `owner_workers = { labourer = 10 }`, because
  the keys of `owner_workers` are the worker kinds a building employs.
- **Labourers** are added on the levels left after any cut to the place's usable land, as the smallest household whose workers reach
  ten a level, and merged into the place's labourer pop (at Armagh, the monastery's). The data's `labourers` is checked against the
  authored levels, not used as the head count.
- **Land order**: holdings, then the demesne, then the farm: the lord's land before the free farmers.

## Real figures and the 768 start (CK-370, characters slice 8)

The game starts with characters (some 2,900 live on the tested seeds): 282 real figures and what the generator makes around them. The three data files are
copies, byte for byte, of the designer's files in `codekarl-content` (`figures/`), and they join the scenario hash (`scenario_hash` reads
all three, the first two found by the scenario's own id, `ireland_768` being `ireland-768.toml` and `ireland-768-load.toml` wherever the folder is, `houses.toml` beside them; all three are required of this scenario, and no other scenario has this start), so a save made with other figures is refused.

| Copy | Source | SHA-256 of the source | Origin |
|---|---|---|---|
| `ireland-768.toml`: 282 figures with their research notes | `codekarl-content/figures/ireland-768.toml` | `744c4fec99dcb21dcb3e49eebaff35d7d65ae9dbce653b674d657cb809ce48ac` | ck-designer-1, 2026-11-16, from `owner-direction-designer-r139` to `-r144`; reviewed on the figures review page |
| `ireland-768-load.toml`: houses, offices, parent and spouse keys, structured dates, `pop_kind`, ten dead parents, 16 strands, 118 slots (r3, change `XAAGSNIT`) | `codekarl-content/figures/ireland-768-load.toml` | `cb8a816459b2997f724d28013ccc8419e26aff8e8dbb5990afa4185af8d87970` | ck-designer-1, 2026-11-20, with the designer's rulings in `history-scratch/ck370-dispositions-r1.md` (SHA-256 `bf5f9e18c8c8e3f11d617be1cd44e8906967a77eed99e8fc2b107bfc2680a13f`) |
| `houses.toml`: the display name of each of the 35 houses and 5 subhouses the load file uses, with the dynasty shown before a name (r1, change `DNSQLPMY`; CK-375) | `codekarl-content/figures/houses.toml` | `c1718937c4ebf04ba4ad805c48252f21078301530d6d56df1d361652fa468c8a` | ck-designer-1, 2026-11-21, from the standard Old Irish spellings of Byrne (*Irish Kings and High-Kings*) and Charles-Edwards (*Early Christian Ireland*); generated minor houses are not listed, their names are made from the founder |

- **Each field has one source.** The research file owns the name, genitive, byname, patronymic, sex, `location`, mother tongue, second
  languages and forced traits (`beats`, `role` and `note` are kept and inert); the load file owns the house, house of birth, subhouse (kept
  for L7), office, parent and spouse keys, `pop_kind` (the place's first noble or monk pop in slot order) and the structured dates. A year
  alone is 1 January. The prose `born`, `died`, `pop` and parent fields of the research file are not read.
- **Dead before the start** (six figures) and the ten dead parents are archived with their dates, never live; a death with no date is
  archived as *unknown*. Generated relatives (r18, the designer's ruling) of a character placed at a **monastic place** (a monk pop and no peasant pop and no
  noble pop: the hermitages and island monasteries, figure or slot member, monk or lay) live at the nearest peasant pop of the character's
  culture (else the nearest labourer pop, else noble), or **abroad** when no non-monk pop of the culture is on the map (Mayo's Northumbrian
  monks); the character stays put. Anywhere else a lay character's relatives live in his own pop (a noble seat keeps its families, with or
  without peasants), and a monk's relatives live in the place's peasant pop (else labourer, else noble). Anyone a slot names, including the
  generated wife who fills a spouse slot, lives where the slot puts them, with her husband.
- **Economy additions** (designer data r2, `history-scratch/ck370-dispositions-r1.md`): a noble pop at Carraig Aille (20 nobles, one demesne
  level, 10 labourers; 34 noble houses, 1,200 nobles and 600 labourer positions in all); Clogher gains a small church (10 monks, 10
  labourers, levels farm 30, holdings 1, storehouse 2); Skellig Michael's monks go from 10 to 12 (an abbot and the traditional twelve).
- **The safety net** (the approved rule that a pop never falls below its characters): at the start only, a pop that would hold fewer heads
  than characters is raised by one dependent instead of refusing a character, and each addition is counted in the start log. The designer's
  join expected it only at Uisneach, by a few; with generator data r2 (generated relatives live to the start by the death curve) it fires at 15
  pops for 91 dependents on seed 7 (Uisneach +14, Ciarraige +11, Aidne and Lagore +10; the start log of a new game lists each pop), `assets/game/lifecycle/generator.toml`.

## Travel network (CK-388, slice 1a)

`network.toml` is the hand-kept list of the 123 links shown in the owner's road mock-ups, approved in designer records r20 and r21.
`network-seed.json` preserves the mock-up courses (`links.json`, SHA-256
`47ecd35f69afbad8046a06f51a1f051a0d267428a12f0f2e78deb2575e0039b4`); the hand-list draft was designer 7's
`network-draft-designer7-r1.toml`, SHA-256 `fd275147dedbbb1ed59f34d0438ef8b2ef897629acae3ca0555798391db5d27c`.
The 24 access qualities follow owner record r41. The great roads start as road level 1; other backbone links as path level 3;
the other paths as path level 1; and the seven shortcuts as path level 0 (owner records r28 and r38).

The worker chose three pins to carry out the moves approved in r43: Dála at 52.672676, -8.494863 to remain on its side of the
Shannon; the Ciarraige path at 52.56, -8.79 for the Maigue crossing at Adare; and the Tory path at 54.96, -7.65 to go around the
head of the Swilly. Inis Cealtra's replacement join on Dála is cell (319, 642); the old completion at (319, 641) is retained as
superseded evidence in the change log. The owner approved the first proposal and its twelve seed completions as the first stored
network in designer record `owner-direction-designer-r75`. The approval does not decide which crossings are fords.

The hand list starts with the mock-up's costs: terrain pasture 1, forest 1.5, mountain 2, marsh 3, bog 3.5; slope 10; fertility
0.5; river crossing base 2 and 0.125 per metre of width; earlier-road reuse 0.6. Route costs use fixed-point arithmetic; the game's
`terrain.png` defines land and the tool does not read `water.png`.

In a copy with no `courses-approved.json` or `roads.toml`, `python3 assets/map/ireland/network.py import` creates the seed baseline
and the first proposal;
`python3 assets/map/ireland/network.py build` proposes later changes. Inspect `network-changes.md` and its linked pictures, obtain
approval, then run `python3 assets/map/ireland/network.py accept --record 'approval record'`. The separate
`python3 assets/map/ireland/network.py check` verifies the stored network without writing it, and `report` prints the stored report. To preview changed costs, use
`build --recalculate LINK_ID` or `build --recalculate --all`. The tool reads Pillow for the map PNGs.

## Water a boat can use, and landings (CK-401, slice 3b)

The stored network is format 3: format 2 with the water. `network.toml` gains the `[water]` table (the one narrowest river, 20 m, for
every boat; shore height 10 m; strait length 500 m), the two `[[water_class]]` entries (`barge`, 1 km, not sea-going, from design
decisions N13 and r56; `currach`, 5 km, sea-going, the designer's proposal), and the two `[[river_break]]` entries (Assaroe on
`erne-44` in cell (360, 243); Doonass on `shannon-18` in cell (318, 642), design decision r39). There is no `[[mark]]` (no place needs
a sea-going boat). The tool reads `terrain.png` and `rivers.toml` and works, once and exactly, the sea-distance layer
`sea_distance.png` (one channel of 16 bits: a sea cell's metres from the nearest cell that is not sea, cut at 65,534; 65,535
elsewhere), the river graph (joints, reaches and joins), the bodies of water of each class, the landings of every node, the ends of
each crossing stretch, and the straits of the ten places that are not islands on this map. `roads.toml` records it all, with the sha256
of `terrain.png`, `rivers.toml` and the layer, and `network-report.md` lists it. `python3 assets/map/ireland/network.py check`
builds it again from the inputs and compares, the layer byte for byte. The owner's decisions are r107 (a place on the coast may take
any shore), r125 (an island's mainland landing works whatever its shore) and r126 (a traveller embarks only at a place, a road
junction, the end of a crossing or an island's mainland landing). The scenario's content hash covers the layer.
