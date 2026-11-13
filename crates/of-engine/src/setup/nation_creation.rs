//! `engine/src/game/NationCreation.ts`. Owner: **P3**. Consumes the **game setup PRNG**
//! (`simpleHash(gameID)`, `GameRunner.ts:56-79`).
//!
//! `manifest_nations` / `additional_nations` are the manifest's `NationSchema` entries carried
//! as `Nation` templates (`map::loader::nation_template`: `spawn_cell` = `coordinates`,
//! `player_info.name`/`nation_flag` = `name`/`flag`; nothing else of the template is read).
//! The name tables are generated from the TS source by `tools/gen_p3_tables.py`.

use crate::api::game_config::GameStartInfo;
use crate::api::types::{Nation, PlayerInfo, PlayerType};
use crate::jsorder::LookupSet;
use crate::setup::nation_count::{NationPick, nation_pick_for_game};
use of_det::Prng;

/// `toNation(n)` (`:29-45`): a fresh `PlayerInfo` with `random.nextID()`.
fn to_nation(n: &Nation, random: &mut Prng) -> Nation {
    let mut info = PlayerInfo::new(n.player_info.name.clone(), PlayerType::Nation, None, random.next_id());
    info.nation_flag = n.player_info.nation_flag.clone(); // `n.flag ?? null`
    Nation { spawn_cell: n.spawn_cell, player_info: info }
}

/// `createNationsForGame(gameStart, manifestNations, additionalNations, numHumans, random)`
/// (`:22-68`).
pub fn create_nations_for_game(
    game_start: &GameStartInfo,
    manifest_nations: &[Nation],
    additional_nations: &[Nation],
    num_humans: u32,
    random: &mut Prng,
) -> Vec<Nation> {
    // int: manifests hold far fewer than 2^32 nations
    let manifest_count = manifest_nations.len() as u32;
    match nation_pick_for_game(&game_start.config, manifest_count, num_humans) {
        NationPick::None => Vec::new(),
        NationPick::Manifest => manifest_nations.iter().map(|n| to_nation(n, random)).collect(),
        NationPick::Random { count } => create_random_nations(count, manifest_nations, additional_nations, random),
    }
}

/// `createRandomNations(targetCount, manifestNations, additionalNations, toNation, random)`
/// (`:76-132`).
fn create_random_nations(
    target_count: u32,
    manifest_nations: &[Nation],
    additional_nations: &[Nation],
    random: &mut Prng,
) -> Vec<Nation> {
    let shuffled = random.shuffle_array(manifest_nations);
    let target = target_count as usize;
    if target <= manifest_nations.len() {
        return shuffled[..target].iter().map(|n| to_nation(n, random)).collect();
    }
    let mut nations: Vec<Nation> = shuffled.iter().map(|n| to_nation(n, random)).collect();
    // lookup-only: `usedNames` is only `has`/`add`ed
    let mut used_names: LookupSet<String> = LookupSet::new();
    for n in &nations {
        used_names.insert(n.player_info.name.clone());
    }
    let mut remaining = target - manifest_nations.len();

    if remaining > 0 && !additional_nations.is_empty() {
        let candidates: Vec<&Nation> =
            additional_nations.iter().filter(|n| !used_names.contains(&n.player_info.name)).collect();
        let shuffled_extras = random.shuffle_array(&candidates);
        let picked = &shuffled_extras[..remaining.min(shuffled_extras.len())];
        for extra in picked {
            let mut info = PlayerInfo::new(extra.player_info.name.clone(), PlayerType::Nation, None, random.next_id());
            info.nation_flag = extra.player_info.nation_flag.clone();
            nations.push(Nation { spawn_cell: extra.spawn_cell, player_info: info });
            used_names.insert(extra.player_info.name.clone());
        }
        remaining -= picked.len();
    }

    for _ in 0..remaining {
        let name = generate_unique_nation_name(random, &used_names);
        used_names.insert(name.clone());
        nations.push(Nation {
            spawn_cell: None,
            player_info: PlayerInfo::new(name, PlayerType::Nation, None, random.next_id()),
        });
    }
    nations
}

/// `generateUniqueNationName(random, usedNames)` (`:134-152`).
fn generate_unique_nation_name(random: &mut Prng, used_names: &LookupSet<String>) -> String {
    for _ in 0..1000 {
        let name = generate_nation_name(random);
        if !used_names.contains(&name) {
            return name;
        }
    }
    let mut counter = 1u64;
    let base_name = generate_nation_name(random);
    while used_names.contains(&format!("{base_name} {counter}")) {
        counter += 1;
    }
    format!("{base_name} {counter}")
}

/// `PLURAL_NOUN` / `NOUN` symbols and literal parts of a `NameTemplate` (`:173-175`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Lit(&'static str),
    Noun,
    Plural,
}

/// `generateNationName(random)` (`:154-171`): template draw, then noun draw.
fn generate_nation_name(random: &mut Prng) -> String {
    // in-range: nextInt(0, len) is in [0, len)
    let template = NAME_TEMPLATES[random.next_int(0.0, NAME_TEMPLATES.len() as f64) as usize];
    let noun = NOUNS[random.next_int(0.0, NOUNS.len() as f64) as usize];
    let parts: Vec<String> = template
        .iter()
        .map(|p| match *p {
            Part::Plural => pluralize(noun),
            Part::Noun => noun.to_owned(),
            Part::Lit(s) => s.to_owned(),
        })
        .collect();
    parts.join(" ")
}

/// `pluralize(noun)` (`:629-655`).
fn pluralize(noun: &str) -> String {
    if let Some(&(_, p)) = SPECIAL_PLURALS.iter().find(|(k, _)| *k == noun) {
        return p.to_owned();
    }
    if noun.ends_with('s') || noun.ends_with("ch") || noun.ends_with("sh") || noun.ends_with('x') || noun.ends_with('z')
    {
        return format!("{noun}es");
    }
    // TS: `noun[noun.length - 2]` (a UTF-16 unit; `undefined` for a 1-unit noun, which
    // `"aeiou".includes` does not contain). The nouns are BMP text.
    let units: Vec<u16> = noun.encode_utf16().collect();
    let before_last = units.len().checked_sub(2).map(|i| units[i]);
    if noun.ends_with('y') && !before_last.is_some_and(|c| "aeiou".encode_utf16().any(|v| v == c)) {
        return format!("{}ies", &noun[..noun.len() - 1]);
    }
    if O_TO_OES.contains(&noun) {
        return format!("{noun}es");
    }
    format!("{noun}s")
}

// @generated-begin nation_name_tables (tools/gen_p3_tables.py; DO NOT EDIT)
/// `NAME_TEMPLATES` (`NationCreation.ts:177-…`).
#[rustfmt::skip]
static NAME_TEMPLATES: &[&[Part]] = &[
    &[Part::Lit("World Famous"), Part::Noun],
    &[Part::Lit("Famous"), Part::Plural],
    &[Part::Lit("Comically Large"), Part::Noun],
    &[Part::Lit("Comically Small"), Part::Noun],
    &[Part::Lit("Massive"), Part::Plural],
    &[Part::Lit("Friendly"), Part::Noun],
    &[Part::Lit("Evil"), Part::Noun],
    &[Part::Lit("Malicious"), Part::Noun],
    &[Part::Lit("Spiteful"), Part::Noun],
    &[Part::Lit("Suspicious"), Part::Noun],
    &[Part::Lit("Canonically Evil"), Part::Noun],
    &[Part::Lit("Limited Edition"), Part::Noun],
    &[Part::Lit("Patent Pending"), Part::Noun],
    &[Part::Lit("Patented"), Part::Noun],
    &[Part::Lit("Space"), Part::Noun],
    &[Part::Lit("Defend The"), Part::Plural],
    &[Part::Lit("Anarchist"), Part::Noun],
    &[Part::Lit("Republic of"), Part::Plural],
    &[Part::Lit("Slippery"), Part::Noun],
    &[Part::Lit("Wealthy"), Part::Plural],
    &[Part::Lit("Certified"), Part::Noun],
    &[Part::Lit("Dr"), Part::Noun],
    &[Part::Lit("Runaway"), Part::Noun],
    &[Part::Lit("Chrome"), Part::Noun],
    &[Part::Lit("All New"), Part::Noun],
    &[Part::Lit("Top Shelf"), Part::Plural],
    &[Part::Lit("Invading"), Part::Plural],
    &[Part::Lit("Loyal To"), Part::Plural],
    &[Part::Lit("United States of"), Part::Noun],
    &[Part::Lit("United States of"), Part::Plural],
    &[Part::Lit("Flowing Rivers of"), Part::Noun],
    &[Part::Lit("House of"), Part::Plural],
    &[Part::Lit("Certified Organic"), Part::Noun],
    &[Part::Lit("Unregulated"), Part::Noun],
    &[Part::Lit("Slightly Damp"), Part::Noun],
    &[Part::Lit("Suspiciously Quiet"), Part::Plural],
    &[Part::Lit("Weaponized"), Part::Noun],
    &[Part::Lit("Accidentally Evil"), Part::Noun],
    &[Part::Lit("Extremely Loud"), Part::Plural],
    &[Part::Lit("Bootleg"), Part::Noun],
    &[Part::Lit("Questionable"), Part::Noun],
    &[Part::Lit("Off-Brand"), Part::Noun],
    &[Part::Lit("Counterfeit"), Part::Plural],
    &[Part::Lit("Sentient"), Part::Plural],
    &[Part::Lit("Feral"), Part::Plural],
    &[Part::Lit("Aggressively Friendly"), Part::Plural],
    &[Part::Lit("Mildly Threatening"), Part::Noun],
    &[Part::Lit("Dangerously Cute"), Part::Plural],
    &[Part::Lit("Legally Distinct"), Part::Noun],
    &[Part::Lit("Deeply Confused"), Part::Plural],
    &[Part::Lit("Order of the"), Part::Noun],
    &[Part::Lit("Knights of the"), Part::Noun],
    &[Part::Lit("Cult of the"), Part::Noun],
    &[Part::Lit("League of"), Part::Plural],
    &[Part::Lit("Band of"), Part::Plural],
    &[Part::Lit("Council of"), Part::Plural],
    &[Part::Lit("Assembly of"), Part::Plural],
    &[Part::Lit("Haunted"), Part::Noun],
    &[Part::Lit("Cursed"), Part::Noun],
    &[Part::Lit("Blessed"), Part::Noun],
    &[Part::Lit("Radioactive"), Part::Plural],
    &[Part::Lit("Deep Fried"), Part::Noun],
    &[Part::Lit("Gluten Free"), Part::Plural],
    &[Part::Lit("Turbocharged"), Part::Noun],
    &[Part::Lit("Nomadic"), Part::Plural],
    &[Part::Lit("Vengeful"), Part::Plural],
    &[Part::Lit("Legendary"), Part::Plural],
    &[Part::Lit("Outlaw"), Part::Plural],
    &[Part::Lit("AFK"), Part::Noun],
    &[Part::Lit("Noob"), Part::Noun],
    &[Part::Lit("Pro"), Part::Noun],
    &[Part::Lit("Tryhard"), Part::Plural],
    &[Part::Lit("Sweaty"), Part::Plural],
    &[Part::Lit("Griefing"), Part::Plural],
    &[Part::Lit("Speedrunning"), Part::Plural],
    &[Part::Lit("Nerfed"), Part::Plural],
    &[Part::Lit("Buffed"), Part::Plural],
    &[Part::Lit("OP"), Part::Noun],
    &[Part::Lit("Overpowered"), Part::Noun],
    &[Part::Lit("Underpowered"), Part::Plural],
    &[Part::Lit("Modded"), Part::Plural],
    &[Part::Lit("Prestige"), Part::Noun],
    &[Part::Lit("Hardcore"), Part::Plural],
    &[Part::Lit("Clutch"), Part::Noun],
    &[Part::Lit("Cracked"), Part::Noun],
    &[Part::Lit("Unranked"), Part::Plural],
    &[Part::Lit("Max Level"), Part::Noun],
    &[Part::Lit("Ironman"), Part::Noun],
    &[Part::Noun, Part::Lit("For Hire")],
    &[Part::Plural, Part::Lit("That Bite")],
    &[Part::Plural, Part::Lit("Are Opps")],
    &[Part::Noun, Part::Lit("Hotel")],
    &[Part::Plural, Part::Lit("The Movie")],
    &[Part::Noun, Part::Lit("Corporation")],
    &[Part::Plural, Part::Lit("Inc")],
    &[Part::Noun, Part::Lit("Democracy")],
    &[Part::Noun, Part::Lit("Network")],
    &[Part::Noun, Part::Lit("Railway")],
    &[Part::Noun, Part::Lit("Congress")],
    &[Part::Noun, Part::Lit("Alliance")],
    &[Part::Noun, Part::Lit("Island")],
    &[Part::Noun, Part::Lit("Kingdom")],
    &[Part::Noun, Part::Lit("Empire")],
    &[Part::Noun, Part::Lit("Dynasty")],
    &[Part::Noun, Part::Lit("Cartel")],
    &[Part::Noun, Part::Lit("Cabal")],
    &[Part::Noun, Part::Lit("Land")],
    &[Part::Noun, Part::Lit("Oligarchy")],
    &[Part::Noun, Part::Lit("Nationalist")],
    &[Part::Noun, Part::Lit("State")],
    &[Part::Noun, Part::Lit("Duchy")],
    &[Part::Noun, Part::Lit("Ocean")],
    &[Part::Noun, Part::Lit("Syndicate")],
    &[Part::Noun, Part::Lit("Republic")],
    &[Part::Noun, Part::Lit("Province")],
    &[Part::Noun, Part::Lit("Dominion")],
    &[Part::Noun, Part::Lit("Commune")],
    &[Part::Noun, Part::Lit("Federation")],
    &[Part::Noun, Part::Lit("Parliament")],
    &[Part::Noun, Part::Lit("Tribunal")],
    &[Part::Noun, Part::Lit("Armada")],
    &[Part::Noun, Part::Lit("Rebellion")],
    &[Part::Noun, Part::Lit("Resistance")],
    &[Part::Noun, Part::Lit("Expedition")],
    &[Part::Noun, Part::Lit("Preservation Society")],
    &[Part::Noun, Part::Lit("Defense League")],
    &[Part::Noun, Part::Lit("Thunderdome")],
    &[Part::Noun, Part::Lit("Uprising")],
    &[Part::Noun, Part::Lit("Enthusiasts")],
    &[Part::Noun, Part::Lit("Appreciation Society")],
    &[Part::Noun, Part::Lit("Fan Club")],
    &[Part::Noun, Part::Lit("Simulation")],
    &[Part::Plural, Part::Lit("Anonymous")],
    &[Part::Plural, Part::Lit("With Attitude")],
    &[Part::Plural, Part::Lit("Gone Wrong")],
    &[Part::Plural, Part::Lit("on Vacation")],
    &[Part::Plural, Part::Lit("in Disguise")],
    &[Part::Plural, Part::Lit("With Hats")],
    &[Part::Plural, Part::Lit("on Ice")],
    &[Part::Plural, Part::Lit("United")],
    &[Part::Plural, Part::Lit("Unhinged")],
    &[Part::Plural, Part::Lit("Unleashed")],
    &[Part::Plural, Part::Lit("Reloaded")],
    &[Part::Plural, Part::Lit("After Dark")],
    &[Part::Plural, Part::Lit("From Space")],
    &[Part::Plural, Part::Lit("of Doom")],
    &[Part::Plural, Part::Lit("Without Borders")],
    &[Part::Noun, Part::Lit("Meta")],
    &[Part::Plural, Part::Lit("OP Please Nerf")],
    &[Part::Lit("Alternate"), Part::Noun, Part::Lit("Universe")],
    &[Part::Lit("Famous"), Part::Noun, Part::Lit("Collection")],
    &[Part::Lit("Supersonic"), Part::Noun, Part::Lit("Spaceship")],
    &[Part::Lit("Secret"), Part::Noun, Part::Lit("Agenda")],
    &[Part::Lit("Ballistic"), Part::Noun, Part::Lit("Missile")],
    &[Part::Lit("The"), Part::Plural, Part::Lit("are SPIES")],
    &[Part::Lit("Traveling"), Part::Noun, Part::Lit("Circus")],
    &[Part::Lit("The"), Part::Plural, Part::Lit("Lied")],
    &[Part::Lit("Sacred"), Part::Noun, Part::Lit("Knowledge")],
    &[Part::Lit("Quantum"), Part::Noun, Part::Lit("Computer")],
    &[Part::Lit("Hadron"), Part::Noun, Part::Lit("Collider")],
    &[Part::Lit("Large"), Part::Noun, Part::Lit("Obliterator")],
    &[Part::Lit("Interstellar"), Part::Noun, Part::Lit("Pirates")],
    &[Part::Lit("Alien"), Part::Noun, Part::Lit("Clan")],
    &[Part::Lit("Grand"), Part::Noun, Part::Lit("Alliance")],
    &[Part::Lit("Royal"), Part::Noun, Part::Lit("Army")],
    &[Part::Lit("Holy"), Part::Noun, Part::Lit("Empire")],
    &[Part::Lit("Eternal"), Part::Noun, Part::Lit("Cabal")],
    &[Part::Lit("Invading"), Part::Noun, Part::Lit("Empire")],
    &[Part::Lit("Immortal"), Part::Noun, Part::Lit("Pirates")],
    &[Part::Lit("Shadow"), Part::Noun, Part::Lit("Cabal")],
    &[Part::Lit("Secret"), Part::Noun, Part::Lit("Dynasty")],
    &[Part::Lit("The Great"), Part::Noun, Part::Lit("Army")],
    &[Part::Lit("The"), Part::Noun, Part::Lit("Matrix")],
    &[Part::Lit("Tax-Free"), Part::Noun, Part::Lit("Paradise")],
    &[Part::Lit("Self-Proclaimed"), Part::Noun, Part::Lit("Experts")],
    &[Part::Lit("Forbidden"), Part::Noun, Part::Lit("Zone")],
    &[Part::Lit("Reluctant"), Part::Noun, Part::Lit("Monarchy")],
    &[Part::Lit("Chaotic"), Part::Noun, Part::Lit("Collective")],
    &[Part::Lit("Unsanctioned"), Part::Noun, Part::Lit("Olympics")],
    &[Part::Lit("The"), Part::Noun, Part::Lit("Conspiracy")],
    &[Part::Lit("The"), Part::Noun, Part::Lit("Incident")],
    &[Part::Lit("The"), Part::Noun, Part::Lit("Situation")],
    &[Part::Lit("Premium"), Part::Noun, Part::Lit("Subscription")],
    &[Part::Lit("Clearance"), Part::Noun, Part::Lit("Warehouse")],
    &[Part::Lit("Budget"), Part::Noun, Part::Lit("Emporium")],
    &[Part::Lit("Overnight"), Part::Noun, Part::Lit("Delivery")],
    &[Part::Lit("National"), Part::Noun, Part::Lit("Reserve")],
    &[Part::Lit("The"), Part::Noun, Part::Lit("Dimension")],
    &[Part::Lit("The"), Part::Noun, Part::Lit("Prophecy")],
    &[Part::Lit("The"), Part::Noun, Part::Lit("Awakening")],
    &[Part::Lit("The"), Part::Noun, Part::Lit("Inquisition")],
    &[Part::Lit("Legendary"), Part::Noun, Part::Lit("Drop")],
    &[Part::Lit("Elite"), Part::Noun, Part::Lit("Squad")],
    &[Part::Lit("The"), Part::Noun, Part::Lit("Saga")],
];
/// `NOUNS` (duplicates kept: they weight the draw).
#[rustfmt::skip]
static NOUNS: &[&str] = &["Snail", "Cow", "Giraffe", "Donkey", "Horse", "Mushroom", "Salad", "Kitten", "Fork", "Apple", "Pancake", "Tree", "Fern", "Seashell", "Turtle", "Casserole", "Gnome", "Frog", "Cheese", "Mold", "Clown", "Boat", "Robot", "Millionaire", "Billionaire", "Pigeon", "Fish", "Bumblebee", "Jelly", "Wizard", "Worm", "Rat", "Pumpkin", "Zombie", "Grass", "Bear", "Skunk", "Sandwich", "Butter", "Soda", "Pickle", "Potato", "Book", "Friend", "Feather", "Flower", "Oil", "Train", "Fan", "Salmon", "Cod", "Sink", "Villain", "Bug", "Car", "Soup", "Puppy", "Rock", "Stick", "Succulent", "Nerd", "Mercenary", "Ninja", "Burger", "Tomato", "Penguin", "Waffle", "Toaster", "Hamster", "Pretzel", "Walrus", "Raccoon", "Llama", "Noodle", "Goblin", "Muffin", "Coconut", "Biscuit", "Cactus", "Moose", "Platypus", "Yeti", "Sponge", "Spatula", "Trampoline", "Dolphin", "Taco", "Chainsaw", "Spoon", "Doorknob", "Bathrobe", "Lampshade", "Crowbar", "Shoelace", "Wheelbarrow", "Barnacle", "Armadillo", "Cabbage", "Wig", "Plunger", "Kazoo", "Napkin", "Pelican", "Turnip", "Canoe", "Igloo", "Stapler", "Ferret", "Anchovy", "Dumpling", "Mattress", "Parsnip", "Gargoyle", "Crayon", "Corgi", "Macaroni", "Blender", "Ukulele", "Flamingo", "Nugget", "Porcupine", "Tadpole", "Papaya", "Chinchilla", "Teapot", "Baguette", "Squid", "Otter", "Badger", "Hedgehog", "Mantis", "Scorpion", "Vulture", "Falcon", "Jackal", "Hyena", "Panther", "Stingray", "Octopus", "Basilisk", "Dragon", "Sphinx", "Phoenix", "Kraken", "Leviathan", "Mammoth", "Chimera", "Griffin", "Minotaur", "Cyclops", "Brick", "Anvil", "Torpedo", "Lantern", "Compass", "Telescope", "Pendulum", "Furnace", "Cauldron", "Beacon", "Anchor", "Dagger", "Gauntlet", "Helmet", "Shield", "Banner", "Trumpet", "Bagpipe", "Tambourine", "Accordion", "Xylophone", "Avocado", "Broccoli", "Radish", "Artichoke", "Kumquat", "Pomegranate", "Mango", "Truffle", "Croissant", "Lasagna", "Soufflé", "Spaghetti", "Tsunami", "Tornado", "Avalanche", "Volcano", "Glacier", "Comet", "Meteor", "Nebula", "Supernova", "Quasar", "Abyss", "Labyrinth", "Caterpillar", "Chameleon", "Narwhal", "Capybara", "Pangolin", "Axolotl", "Sloth", "Lemur", "Alpaca", "Tapir", "Wombat", "Ocelot", "Manatee", "Ibis", "Kiwi", "Creeper", "Enderman", "Skeleton", "Necromancer", "Paladin", "Warlock", "Ranger", "Boss", "NPC", "Assassin", "Viking", "Samurai", "Pirate", "Champion", "Gladiator", "Demon", "Angel", "Fullsender", "Fullsender", "Fullsender", "Mito", "Mito", "Mito", "Mitochondria", "Mitochondria", "Mitochondria"];
/// `O_TO_OES` (`new Set([...])`).
#[rustfmt::skip]
static O_TO_OES: &[&str] = &["Potato", "Tomato", "Volcano", "Torpedo"];
/// `SPECIAL_PLURALS` (`new Map([...])`).
#[rustfmt::skip]
static SPECIAL_PLURALS: &[(&str, &str)] = &[("Cactus", "Cacti"), ("Platypus", "Platypuses"), ("Moose", "Moose"), ("Octopus", "Octopi"), ("Cyclops", "Cyclopes"), ("Samurai", "Samurai"), ("Fish", "Fish"), ("Salmon", "Salmon"), ("Cod", "Cod"), ("Enderman", "Endermen"), ("Mitochondria", "Mitochondria")];
// @generated-end nation_name_tables

#[cfg(test)]
#[path = "nation_creation_tests.rs"]
mod tests;
