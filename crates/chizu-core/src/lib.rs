//! chizu のシミュレーションの中核。

pub mod chunk;
pub mod clock;
pub mod command;
pub mod record;
pub mod rng;
pub mod round;
pub mod state_hash;
pub mod terrain;
pub mod tile;
pub mod units;
pub mod world;

pub use chunk::{Chunk, Layer, LayerTiles, TileUse};
pub use clock::{GameTime, TICKS_PER_DAY};
pub use command::{Command, NodeId, RoadKind, ServiceKind, SimSpeed, ZoneKind};
pub use record::{CommandLog, DecodeError, RecordedCommand, ReplayStats, replay};
pub use rng::{Pcg32, RngPurpose, mix_words, rng};
pub use state_hash::{StateHash, StateHasher, state_hash_of};
pub use terrain::{TerrainTiles, is_water, terrain};
pub use tile::{CHUNK_AREA, CHUNK_SIDE, ChunkCoord, TilePos, TileRect};
pub use units::{LaneOffset, Mm, MmPerTick, Money, Tick, WorldPos};
pub use world::{ApplyOutcome, World};
