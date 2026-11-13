//! `engine-lib/src/execution/utils/TribeNames.ts`. Owner: **P3**.
//!
//! The themes table is generated from `resources/tribeNameThemes.json` by
//! `tools/gen_p3_tables.py`; the per-map `themes`/`customTribes` come from `map::maps_gen`.

use crate::map::manifest::CustomTribe;
use crate::map::maps_gen;

/// `TribeNameData` (`:9-13`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TribeNameData {
    pub prefixes: Vec<String>,
    pub suffixes: Vec<String>,
    pub custom_tribes: Option<Vec<CustomTribe>>,
}

/// `tribeNameThemes[name]` (`Record<string, TribeNameTheme>` lookup).
fn theme(name: &str) -> Option<(&'static [&'static str], &'static [&'static str])> {
    TRIBE_NAME_THEMES.iter().find(|(k, _, _)| *k == name).map(|&(_, p, s)| (p, s))
}

fn extend(v: &mut Vec<String>, xs: &[&str]) {
    v.extend(xs.iter().map(|s| (*s).to_owned()));
}

/// `resolveTribeNameData(mapType?)` (`:35-93`).
pub fn resolve_tribe_name_data(map_type: Option<&str>) -> TribeNameData {
    if let Some(mt) = map_type
        && let Some(info) = maps_gen::map_info(mt)
    {
        let theme_names: &[&str] = match info.themes {
            Some(t) if !t.is_empty() => t,
            _ => &["default"],
        };
        let mut prefixes = Vec::new();
        let mut suffixes = Vec::new();
        for &name in theme_names {
            // TS: unknown theme → console.warn + skip
            let Some((p, s)) = theme(name) else { continue };
            extend(&mut prefixes, p);
            extend(&mut suffixes, s);
        }
        // TS: `:59-68` all themes unknown → append the default theme
        if prefixes.is_empty() || suffixes.is_empty() {
            let Some((p, s)) = theme("default") else {
                of_det::ts_throw!("[TribeNames] Default theme is missing from tribeNameThemes.json")
            };
            extend(&mut prefixes, p);
            extend(&mut suffixes, s);
        }
        let custom_tribes = match info.custom_tribes {
            Some(c) if !c.is_empty() => Some(
                c.iter().map(|t| CustomTribe::Placed { name: t.name.to_owned(), coordinates: t.coordinates }).collect(),
            ),
            _ => None,
        };
        return TribeNameData { prefixes, suffixes, custom_tribes };
    }
    // No map type or map not found: the default theme.
    let Some((p, s)) = theme("default") else {
        of_det::ts_throw!("[TribeNames] Default theme is missing from tribeNameThemes.json")
    };
    TribeNameData {
        prefixes: p.iter().map(|s| (*s).to_owned()).collect(),
        suffixes: s.iter().map(|s| (*s).to_owned()).collect(),
        custom_tribes: None,
    }
}

// @generated-begin tribe_name_themes (tools/gen_p3_tables.py; DO NOT EDIT)
/// `resources/tribeNameThemes.json`: `(theme, prefixes, suffixes)` in file order.
#[rustfmt::skip]
static TRIBE_NAME_THEMES: &[(&str, &[&str], &[&str])] = &[
    ("default", &["Akkadian", "Babylonian", "Sumerian", "Hittite", "Phoenician", "Canaanite", "Minoan", "Mycenaean", "Etruscan", "Scythian", "Thracian", "Dacian", "Illyrian", "Median", "Chaldean", "Roman", "Greek", "Byzantine", "Persian", "Parthian", "Seleucid", "Ptolemaic", "Palmyrene", "Macedonian", "Carthaginian", "Ming", "Tang", "Song", "Yuan", "Mauryan", "Kushan", "Rajput", "Mughal", "Satavahana", "Vijayanagara", "Egyptian", "Nubian", "Kushite", "Aksumite", "Ethiopian", "Songhai", "Malian", "Ghanaian", "Benin", "Ashanti", "Zulu", "Tuareg", "Berber", "Kanem-Bornu", "Buganda", "Mossi", "Swahili", "Somali", "Wolof", "Umayyad", "Abbasid", "Ayyubid", "Fatimid", "Mamluk", "Seljuk", "Safavid", "Ottoman", "Almoravid", "Almohad", "Rashidun", "Ziyarid", "Frankish", "Visigothic", "Ostrogothic", "Viking", "Norman", "Saxon", "Anglo-Saxon", "Celtic", "Gaulish", "Carolingian", "Merovingian", "Capetian", "Plantagenet", "Tudor", "Stuart", "Habsburg", "Romanov", "Lancaster", "York", "Bourbon", "Napoleonic", "British", "French", "Spanish", "Portuguese", "Dutch", "Russian", "German", "Italian", "Swedish", "Norwegian", "Danish", "Polish", "Hungarian", "Austrian", "Swiss", "Czech", "Slovak", "Serbian", "Croatian", "Bosnian", "Montenegrin", "Bulgarian", "Romanian", "Apache", "Sioux", "Cherokee", "Navajo", "Iroquois", "Inuit", "Arawak", "Carib", "Taino", "Aztec", "Mayan", "Incan", "Mapuche", "Guarani", "Tupi", "Yanomami", "Zuni", "Hopi", "Kiowa", "Comanche", "Shoshone", "Japanese", "Ryukyu", "Ainu", "Cham", "Khmer", "Thai", "Vietnamese", "Burmese", "Balinese", "Malay", "Filipino", "Mongolian", "Korean", "Tibetan", "Manchu", "Uyghur", "Hmong", "Karen", "Pyu", "Hawaiian", "Fijian", "Tongan", "Samoan", "Maori", "Micronesian", "Hebrew", "Armenian", "Circassian", "Georgian", "Kurdish", "Turkic", "Kazakh", "Uzbek", "Kyrgyz", "Tajik", "Pashtun", "Baloch", "Afghan", "Kenyan", "Ugandan", "Bhutanese", "Latin", "Moldovan", "Militant", "Spartan"], &["Empire", "Dynasty", "Kingdom", "Queendom", "Sultanate", "Confederation", "Union", "Republic", "Caliphate", "Dominion", "Realm", "State", "Federation", "Territory", "Commonwealth", "League", "Duchy", "Province", "Protectorate", "Colony", "Mandate", "Free State", "Canton", "Region", "Nation", "Assembly", "Hierarchy", "Archduchy", "Grand Duchy", "Metropolis", "Cluster", "Alliance", "Tribunal", "Council", "Confederacy", "Order", "Regime", "Syndicate", "Guild", "Corporation", "Patriarchy", "Matriarchy", "Legion", "Horde", "Clan", "Brotherhood", "Sisterhood", "Ascendancy", "Supremacy", "Tribe", "Army", "Dictatorship", "Country", "Oligarchy", "Monkdom", "Throng", "Host", "Area", "District", "Fief", "Wilderness", "Settlement", "Parliament", "Anarchy", "Democracy", "Autocracy"]),
    ("north_america", &["Apache", "Sioux", "Cherokee", "Navajo", "Iroquois", "Inuit", "Arawak", "Carib", "Taino", "Aztec", "Mayan", "Zuni", "Hopi", "Kiowa", "Comanche", "Shoshone", "Cree", "Ojibwe", "Seminole", "Cheyenne", "Pawnee", "Mohawk", "Huron", "Tlingit", "Haida", "Nez Perce", "Crow", "Blackfoot", "Pueblo", "Mi'kmaq", "Algonquin", "Wampanoag", "Potawatomi", "Shawnee", "Chickasaw", "Choctaw", "Creek", "Canadian", "American", "Mexican", "Texan", "Californian", "Quebecois", "Acadian", "Louisianan", "Appalachian", "Yankee", "Frontiersman"], &["Nation", "Confederation", "Territory", "Federation", "Republic", "State", "Province", "Colony", "Commonwealth", "Dominion", "Council", "Assembly", "Tribe", "Clan", "League", "Alliance", "Union", "Band", "Settlement", "Frontier", "Range", "Heartland", "Homeland", "Territories"]),
    ("south_america", &["Incan", "Mapuche", "Guarani", "Tupi", "Yanomami", "Quechua", "Aymara", "Chibcha", "Muisca", "Taíno", "Carib", "Wari", "Chimu", "Nazca", "Moche", "Olmec", "Zapotec", "Mixtec", "Toltec", "Brazilian", "Argentine", "Chilean", "Colombian", "Peruvian", "Venezuelan", "Ecuadorian", "Bolivian", "Paraguayan", "Uruguayan", "Guyanese", "Surinamese", "Patagonian", "Amazonian", "Andean", "Gaucho", "Carioca", "Porteño", "Cordobés", "Bogotano", "Limeño"], &["Empire", "Kingdom", "Republic", "Confederation", "Federation", "State", "Nation", "Territory", "Province", "Chiefdom", "Dynasty", "Alliance", "League", "Council", "Tribe", "Clan", "Collective", "Domain", "Realm", "Heartland", "Amazonia", "Patagonia", "Andes", "Pampas"]),
    ("europe", &["Roman", "Greek", "Byzantine", "Frankish", "Visigothic", "Ostrogothic", "Viking", "Norman", "Saxon", "Anglo-Saxon", "Celtic", "Gaulish", "Carolingian", "Merovingian", "Capetian", "Plantagenet", "Tudor", "Stuart", "Habsburg", "Romanov", "Lancaster", "York", "Bourbon", "Napoleonic", "British", "French", "Spanish", "Portuguese", "Dutch", "Russian", "German", "Italian", "Swedish", "Norwegian", "Danish", "Polish", "Hungarian", "Austrian", "Swiss", "Czech", "Slovak", "Serbian", "Croatian", "Bosnian", "Montenegrin", "Bulgarian", "Romanian", "Finnish", "Estonian", "Latvian", "Lithuanian", "Irish", "Scottish", "Welsh", "Basque", "Galician", "Sardinian", "Sicilian", "Venetian", "Florentine", "Genoese", "Milanese", "Prussian", "Bavarian", "Teutonic", "Knights Templar", "Crusader", "Papal", "Holy Roman"], &["Kingdom", "Empire", "Republic", "Duchy", "Grand Duchy", "Archduchy", "County", "Principality", "Barony", "Lordship", "March", "Shire", "Fief", "Realm", "Dominion", "Crown", "Dynasty", "House", "Order", "League", "Confederation", "Alliance", "Pact", "Union", "Federation", "Commonwealth", "State", "Nation", "Province", "Territory"]),
    ("africa", &["Egyptian", "Nubian", "Kushite", "Aksumite", "Ethiopian", "Songhai", "Malian", "Ghanaian", "Benin", "Ashanti", "Zulu", "Tuareg", "Berber", "Kanem-Bornu", "Buganda", "Mossi", "Swahili", "Somali", "Wolof", "Yoruba", "Igbo", "Hausa", "Fulani", "Maasai", "Kikuyu", "Shona", "Luba", "Lunda", "Kongo", "Lozi", "Tswana", "Sotho", "Xhosa", "Rwandan", "Burundian", "Baganda", "Bamileke", "Fang", "San", "Khoikhoi", "Merina", "Sakalava", "Betsileo", "Matabele", "Rozvi", "Mutapa"], &["Empire", "Kingdom", "Sultanate", "Caliphate", "Dynasty", "Chiefdom", "Confederation", "Republic", "State", "Territory", "Province", "Region", "Nation", "Tribe", "Clan", "Alliance", "League", "Council", "Assembly", "Federation", "Union", "Dominion", "Realm", "Colony", "Protectorate"]),
    ("asia", &["Ming", "Tang", "Song", "Yuan", "Mauryan", "Kushan", "Rajput", "Mughal", "Satavahana", "Vijayanagara", "Japanese", "Ryukyu", "Ainu", "Cham", "Khmer", "Thai", "Vietnamese", "Burmese", "Balinese", "Malay", "Filipino", "Mongolian", "Korean", "Tibetan", "Manchu", "Uyghur", "Hmong", "Karen", "Pyu", "Han", "Qin", "Jin", "Sui", "Zhou", "Shang", "Xia", "Choson", "Goguryeo", "Baekje", "Silla", "Goryeo", "Joseon", "Yamato", "Heian", "Tokugawa", "Ashikaga", "Kamakura", "Fujiwara", "Taira", "Minamoto", "Srivijayan", "Majapahit", "Mataram", "Ayutthayan", "Sukhothai", "Lanna", "Champa", "Dai Viet", "Khitan", "Jurchen", "Xiongnu", "Göktürk", "Tangut", "Timurid", "Gupta", "Pala", "Chola", "Pandya", "Rashtrakuta", "Chalukya", "Hoysala", "Kalinga"], &["Dynasty", "Empire", "Kingdom", "Shogunate", "Sultanate", "Khanate", "Principality", "Republic", "State", "Confederation", "Horde", "Realm", "Province", "Commandery", "Satrapy", "Viceroyalty", "Mandate", "Regency", "Clan", "Domain", "Fief", "March", "Colony", "Protectorate", "Tributary"]),
    ("antarctica", &["Australian", "New Zealand", "Argentine", "Chilean", "French", "Norwegian", "British", "American", "Russian", "Soviet", "Transantarctic", "Antarctic", "Southern", "South Sandwich", "South Georgia", "Kerguelen", "Scotia", "Weddell", "Lazarev", "Riisen-Larsen", "Cosmonauts", "Cooperation", "Penguin", "Mawson", "D'Urville", "Somov", "Ross", "Amundsen", "Bellingshausen", "Trinity", "Palmer", "Marie Byrd", "Enderby", "Ellsworth", "Victoria", "Wilkes", "Queen Maud", "Queen Mary", "Elizabeth", "Balleny", "Pinniped", "Heard", "McDonald", "Bouvet", "Adelaide", "Magellanic", "Crozet", "Berkner", "Prince Edward", "Vostok", "Glacial", "Subglacial", "Whaler", "Magnetic"], &["Base", "Nation", "Federation", "Confederation", "Kingdom", "Republic", "Plateau", "Coast", "Voyage", "Territory", "Pole", "Empire", "League", "Union", "Domain", "Realm", "Colony", "Station", "Claim", "Clan", "Tribe", "Shelf", "Glacier", "Expedition", "Land"]),
    ("oceania", &["Hawaiian", "Fijian", "Tongan", "Samoan", "Maori", "Micronesian", "Polynesian", "Melanesian", "Papuan", "Aboriginal", "Torres Strait", "Australian", "New Zealand", "Tahitian", "Marquesan", "Rapa Nui", "Chamorro", "Carolinian", "Marshallese", "Palauan", "Nauruan", "Tuvaluan", "Niuean", "Cook Islander", "Tokelauan", "Wallisian", "Futunan", "New Caledonian", "Vanuatuan", "Solomon Islander", "Bougainvillean"], &["Islands", "Nation", "Federation", "Confederation", "Kingdom", "Republic", "Commonwealth", "Territory", "Council", "Alliance", "League", "Union", "Domain", "Realm", "Parliament", "Assembly", "Tribal Council", "Clan", "Tribe", "Homelands", "Archipelago", "Atoll", "Lagoon"]),
    ("space", &["Andromedan", "Centaurian", "Sirian", "Arcturian", "Pleiadian", "Vegan", "Lyran", "Orion", "Antarian", "Procyon", "Aldebaran", "Rigelian", "Betelgeusian", "Denebian", "Altairian", "Fomalhautian", "Capellan", "Zeta", "Tau", "Epsilon", "Keplerian", "Gliesean", "Trappist", "Proximan", "Barnardian", "Wolfian", "Luytenian", "Rossian", "Cygnian", "Aquilan", "Draconian", "Pegasian", "Hydran", "Corvan", "Nebulan", "Quasar", "Pulsar", "Novan", "Stellar", "Cosmic", "Galactic", "Lunar", "Solar", "Astral", "Voidborn", "Starforged", "Nebulite", "Quantum", "Dark Matter"], &["Dominion", "Empire", "Federation", "Confederacy", "Commonwealth", "Collective", "Hive", "Alliance", "Coalition", "Concordat", "Protectorate", "Mandate", "Territory", "Sector", "Cluster", "Nebula", "Constellation", "Armada", "Fleet", "Legion", "Order", "Syndicate", "Corporation", "Council", "Tribunal", "Ascendancy", "Supremacy", "Swarm", "Horde", "Brotherhood"]),
    ("fantasy", &["Dragon", "Elven", "Dwarven", "Orcish", "Goblin", "Fairy", "Shadow", "Crystal", "Storm", "Iron", "Steel", "Golden", "Silver", "Emerald", "Ruby", "Sapphire", "Obsidian", "Moonlit", "Starlit", "Twilight", "Dawn", "Dusk", "Eternal", "Ancient", "Arcane", "Mystic", "Enchanted", "Cursed", "Blessed", "Sacred", "Dark", "Light", "Frost", "Flame", "Thorn", "Raven", "Wolf", "Bear", "Eagle", "Lion", "Serpent", "Phoenix", "Griffin", "Unicorn", "Pegasus", "Troll", "Giant", "Fey", "Sylvan", "Abyssal", "Celestial", "Infernal", "Primordial", "Runed", "Warded"], &["Kingdom", "Realm", "Dominion", "Empire", "Dynasty", "Order", "Covenant", "Pact", "Alliance", "Guild", "Circle", "Council", "Conclave", "Sanctum", "Citadel", "Throne", "Crown", "Spire", "Hollow", "Reach", "Bastion", "Haven", "Enclave", "Brotherhood", "Sisterhood", "Horde", "Legion", "Swarm", "Host", "House", "Clan", "Tribe", "Nation", "Domain"]),
    ("war", &["Iron", "Steel", "Thunder", "Lightning", "Blitz", "Vanguard", "Phalanx", "Spartan", "Gladiator", "Centurion", "Hussar", "Cossack", "Samurai", "Mongol", "Hoplite", "Legionary", "Musketeer", "Dragoon", "Lancer", "Commando", "Ranger", "Sentinel", "Warden", "Reaver", "Berserker", "Juggernaut", "Valkyrie", "Warlord", "Overlord", "General", "Marshal", "Captain", "Sergeant", "Colonel", "Admiral", "Commodore", "Tactician", "Strategist", "Warmonger", "Conqueror", "Liberator", "Insurgent", "Partisan", "Guerrilla", "Militia", "Mercenary", "Veteran", "Elite"], &["Legion", "Army", "Corps", "Division", "Brigade", "Battalion", "Regiment", "Company", "Platoon", "Squad", "Phalanx", "Bastion", "Front", "Theater", "Warband", "Host", "Horde", "Force", "Command", "Operations", "Strike", "Assault", "Defense", "Garrison", "Outpost", "Fortress", "Citadel", "Arsenal", "Armory", "War Council", "High Command", "Tribunal"]),
    ("western", &["Cowboy", "Outlaw", "Sheriff", "Marshal", "Ranger", "Desperado", "Gunslinger", "Bounty Hunter", "Frontiersman", "Pioneer", "Prospector", "Rancher", "Bandit", "Renegade", "Drifter", "Wrangler", "Vaquero", "Buckaroo", "Lawman", "Deputy", "Scout", "Trapper", "Miner", "Preacher", "Doc", "Dusty", "Lonesome", "Wild", "Iron", "Silver", "Gold", "Copper", "Dakota", "Texas", "Arizona", "Montana", "Nevada", "Colorado", "Wyoming", "Oklahoma", "Kansas", "Nebraska", "New Mexico", "Deadwood", "Tombstone"], &["Territory", "Frontier", "Range", "County", "District", "Settlement", "Outpost", "Trading Post", "Ranch", "Claim", "Camp", "Crossing", "Pass", "Gulch", "Canyon", "Ridge", "Valley", "Springs", "Flats", "Hollow", "Posse", "Gang", "Cartel", "Syndicate", "Alliance", "Confederation", "Sheriff's Office", "Marshal's Office", "Wagon Train", "Cattle Drive"]),
    ("under_ocean", &["Atlantean", "Abyssal", "Coral", "Deep", "Leviathan", "Kraken", "Nautilus", "Trident", "Pearl", "Tidal", "Submarine", "Bathyal", "Hadal", "Pelagic", "Neritic", "Benthic", "Abyssopelagic", "Mesopelagic", "Epipelagic", "Siren", "Merfolk", "Triton", "Poseidon", "Neptune", "Nereid", "Scylla", "Charybdis", "Lusca", "Jormungandr", "Tiamat", "Naga", "Selkie", "Kelpie", "Bunyip", "Djinn", "Undine", "Nixie", "Rusalka"], &["Kingdom", "Depth", "Abyss", "Trench", "Reef", "Lagoon", "Atoll", "Archipelago", "Domain", "Realm", "Empire", "Dynasty", "Confederation", "Federation", "Collective", "Colony", "Hive", "Swarm", "School", "Council", "Conclave", "Order", "Brotherhood", "Sisterhood", "Horde", "Legion", "Host", "Current", "Tide", "Depths", "Waters", "Seas"]),
    ("tournament", &["Champion", "Knight", "Gladiator", "Warrior", "Paladin", "Hero", "Defender", "Guardian", "Protector", "Avenger", "Slayer", "Conqueror", "Victor", "Legend", "Titan", "Colossus", "Goliath", "Hercules", "Achilles", "Odysseus", "Leonidas", "Alexander", "Caesar", "Attila", "Genghis", "Napoleon", "Valor", "Honor", "Glory", "Triumph", "Dominance", "Supremacy", "Invincible", "Unstoppable", "Relentless", "Fearless", "Ruthless", "Merciless", "Undying", "Immortal", "Eternal", "Ascendant", "Apex"], &["Arena", "Colosseum", "Ring", "Tournament", "Championship", "League", "Series", "Circuit", "Grand Prix", "Classic", "Invitational", "Open", "Cup", "Trophy", "Prize", "Crown", "Laurels", "Medal", "Award", "Honor Guard", "Elite", "Champions", "Masters", "Grandmasters", "Titans", "Legends", "Hall of Fame", "Order of Merit", "Circle"]),
    ("funny", &["Silly", "Wacky", "Zany", "Goofy", "Bonkers", "Loony", "Nutty", "Kooky", "Quirky", "Batty", "Daffy", "Dizzy", "Loopy", "Wobbly", "Funky", "Groovy", "Spunky", "Peppy", "Bouncy", "Jolly", "Merry", "Cheerful", "Giggly", "Snicker", "Chuckle", "Tickle", "Pickle", "Noodle", "Waffle", "Pancake", "Muffin", "Cupcake", "Donut", "Taco", "Burrito", "Nacho", "Pretzel", "Bagel", "Cookie", "Biscuit", "Crumpet", "Scone", "Pudding", "Custard", "Trifle", "Jellybean", "Gummy", "Lollipop"], &["Squad", "Team", "Club", "Gang", "Crew", "Bunch", "Pack", "Mob", "Horde", "Swarm", "Flock", "Herd", "School", "Colony", "Band", "Troupe", "Ensemble", "Posse", "Clique", "Circle", "Society", "Association", "Fellowship", "Brotherhood", "Sisterhood", "Alliance", "Coalition", "League", "Union", "Federation", "Confederation", "Collective"]),
    ("scary", &["Dark", "Shadow", "Nightmare", "Ghoul", "Phantom", "Specter", "Wraith", "Revenant", "Banshee", "Dread", "Terror", "Horror", "Malice", "Venom", "Plague", "Pestilence", "Famine", "Death", "Doom", "Ruin", "Blight", "Curse", "Hex", "Jinx", "Spell", "Charm", "Hexen", "Witch", "Warlock", "Necromancer", "Lich", "Vampire", "Werewolf", "Zombie", "Skeleton", "Corpse", "Carrion", "Rot", "Decay", "Void", "Abyss", "Oblivion", "Entropy", "Chaos", "Madness", "Insanity", "Dementia", "Psychosis"], &["Horde", "Legion", "Swarm", "Plague", "Scourge", "Blight", "Curse", "Nightmare", "Terror", "Dread", "Fear", "Panic", "Ruin", "Doom", "Death", "Reaping", "Harvest", "Culling", "Purge", "Extermination", "Brood", "Nest", "Lair", "Den", "Crypt", "Tomb", "Catacombs", "Coven", "Circle", "Order", "Cult", "Sect", "Brotherhood", "Pact"]),
    ("weird", &["Bizarre", "Strange", "Odd", "Peculiar", "Weird", "Eerie", "Uncanny", "Surreal", "Absurd", "Grotesque", "Macabre", "Eldritch", "Ethereal", "Spectral", "Gossamer", "Translucent", "Iridescent", "Opalescent", "Prismatic", "Kaleidoscopic", "Phantasmagoric", "Ineffable", "Inscrutable", "Paradoxical", "Impossible", "Inconceivable", "Unfathomable", "Inexplicable", "Anomalous", "Aberrant", "Deviant", "Atypical", "Unorthodox", "Heterodox", "Nonconformist", "Maverick", "Rogue", "Rebel", "Dissident", "Heretic"], &["Collective", "Hive", "Swarm", "Network", "Matrix", "Web", "Lattice", "Grid", "Array", "Spectrum", "Prism", "Kaleidoscope", "Paradox", "Anomaly", "Aberration", "Enigma", "Mystery", "Riddle", "Puzzle", "Conundrum", "Dilemma", "Quandary", "Oxymoron", "Contradiction", "Dichotomy", "Juxtaposition", "Irony", "Satire", "Absurdity", "Nonsense"]),
    ("vs", &["Red", "Blue", "Green", "Yellow", "Orange", "Purple", "Pink", "Black", "White", "Grey", "Gold", "Silver", "Bronze", "Copper", "Iron", "Steel", "Alpha", "Beta", "Gamma", "Delta", "Epsilon", "Zeta", "Eta", "Theta", "North", "South", "East", "West", "Dawn", "Dusk", "Day", "Night", "Fire", "Water", "Earth", "Air", "Lightning", "Ice", "Nature", "Shadow", "Light", "Dark", "Order", "Chaos", "Life", "Death", "War", "Peace"], &["Team", "Squad", "Crew", "Division", "Brigade", "Regiment", "Battalion", "Company", "Platoon", "Unit", "Force", "Command", "Operations", "Strike", "Assault", "Defense", "Vanguard", "Rearguard", "Flank", "Center", "Wing", "Corps", "Legion", "Army", "Horde", "Host", "Alliance", "Coalition", "League", "Union", "Federation", "Confederation"]),
];
// @generated-end tribe_name_themes

/// Port of `upstream/tests/TribeNameThemes.test.ts` (the `resolveTribeNameData` and
/// `tribeNameThemes.json` describes; `createRandomName` is client code, not ported).
#[cfg(test)]
mod tests {
    use super::*;

    fn distinct(v: &[String]) -> bool {
        let mut s = crate::jsorder::LookupSet::new();
        v.iter().all(|x| s.insert(x.clone()))
    }

    // TS: TribeNameThemes.test.ts:7,19
    #[test]
    fn default_theme_without_map() {
        let d = resolve_tribe_name_data(None);
        assert!(!d.prefixes.is_empty() && !d.suffixes.is_empty());
        assert!(d.custom_tribes.is_none());
        for p in ["Roman", "Viking"] {
            assert!(d.prefixes.iter().any(|x| x == p));
        }
        for s in ["Empire", "Kingdom"] {
            assert!(d.suffixes.iter().any(|x| x == s));
        }
    }

    // TS: :25 (World has no customTribes)
    #[test]
    fn world_has_names_and_no_custom_tribes() {
        let d = resolve_tribe_name_data(Some("World"));
        assert!(!d.prefixes.is_empty() && !d.suffixes.is_empty());
        assert!(d.custom_tribes.is_none());
    }

    // TS: :33
    #[test]
    fn every_game_map_type_resolves() {
        for (_, v) in maps_gen::GAME_MAP_TYPES {
            let d = resolve_tribe_name_data(Some(v));
            assert!(!d.prefixes.is_empty() && !d.suffixes.is_empty(), "{v}");
        }
        // themes merge in theme order: Achiran = europe + scary
        let d = resolve_tribe_name_data(Some("Achiran"));
        let (ep, es) = theme("europe").unwrap();
        let (sp, ss) = theme("scary").unwrap();
        assert_eq!(d.prefixes.len(), ep.len() + sp.len());
        assert_eq!(d.suffixes.len(), es.len() + ss.len());
        assert_eq!(d.prefixes[0], ep[0]);
        assert_eq!(d.prefixes[ep.len()], sp[0]);
        // a map with custom tribes keeps them in Maps.gen order
        let n = maps_gen::MAPS.iter().filter(|m| m.custom_tribes.is_some()).count();
        assert!(n > 0);
        for m in maps_gen::MAPS.iter().filter(|m| m.custom_tribes.is_some()) {
            let ct = resolve_tribe_name_data(Some(m.map_type)).custom_tribes.unwrap();
            let want = m.custom_tribes.unwrap();
            assert_eq!(ct.len(), want.len());
            assert_eq!(ct[0], CustomTribe::Placed { name: want[0].name.to_owned(), coordinates: want[0].coordinates });
        }
        // an unknown map name → default theme
        assert_eq!(resolve_tribe_name_data(Some("No Such Map")), resolve_tribe_name_data(None));
    }

    // TS: :41,51,65,71
    #[test]
    fn default_theme_entries_non_empty_and_distinct() {
        let d = resolve_tribe_name_data(None);
        assert!(d.prefixes.iter().chain(&d.suffixes).all(|x| !x.is_empty()));
        assert!(distinct(&d.prefixes) && distinct(&d.suffixes));
        let mut r = of_det::Prng::new(42.0);
        for _ in 0..50 {
            let p = &d.prefixes[r.next_int(0.0, d.prefixes.len() as f64) as usize];
            let s = &d.suffixes[r.next_int(0.0, d.suffixes.len() as f64) as usize];
            let name = format!("{p} {s}");
            assert!(name.contains(' '));
        }
    }

    // TS: :82,88,96,108
    #[test]
    fn themes_json_shape() {
        assert!(theme("default").is_some());
        assert!(TRIBE_NAME_THEMES.len() >= 3);
        for (k, p, s) in TRIBE_NAME_THEMES {
            assert!(!p.is_empty() && !s.is_empty(), "{k}");
            assert!(p.iter().chain(s.iter()).all(|x| !x.is_empty()), "{k}");
        }
    }
}
