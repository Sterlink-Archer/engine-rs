//! Game setup: nations, teams, tribe names and spawners (FILE_MAP rows 24, 27, 28, 33, 75, 79).
//! Owner of this file: **P1** (the module list only; adding a module is a P1 one-line PR).
//! The modules belong to P3 (nation_count, team_assignment, nation_creation, tribe_names) and
//! P6 (tribe_spawner, player_spawner), as their own headers say.

pub mod nation_count;
pub mod nation_creation;
pub mod player_spawner;
pub mod team_assignment;
pub mod tribe_names;
pub mod tribe_spawner;
