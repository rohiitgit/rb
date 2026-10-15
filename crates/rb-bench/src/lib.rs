//! Repository Brain accuracy benchmark (contract M8a).
//!
//! Pure functions over `rb dump` text and answer keys: [`dump`] reads a dump into rb-model's own
//! row types, [`identity`] reduces symbols and edges to a location identity that is the same in
//! every indexing mode and in external answer keys, and [`score`] places every gold item and every
//! found edge in exactly one bucket. [`scoreboard`] stores the counts, [`render`] derives the
//! tables and [`compare`] is the gate. [`scip_ref`] is the SCIP agreement key; [`run`] turns a
//! `bench/run.sh` output directory into a scoreboard. Indexing, cloning and timing are
//! `bench/*.sh`.

pub mod compare;
pub mod dump;
pub mod handkey;
pub mod identity;
pub mod msecend;
pub mod pycg;
pub mod render;
pub mod run;
pub mod sample;
pub mod scip_ref;
pub mod score;
pub mod scoreboard;
pub mod structural;
