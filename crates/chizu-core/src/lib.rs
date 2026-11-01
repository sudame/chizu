//! chizu のシミュレーションの中核。

pub mod building;
pub mod chunk;
pub mod citizens;
pub mod clock;
pub mod command;
pub mod id;
pub mod network;
pub mod record;
pub mod rng;
pub mod round;
pub mod routing;
pub mod schedule;
pub mod state_hash;
pub mod terrain;
pub mod tile;
pub mod units;
pub mod world;
pub mod zone;

pub use chunk::{Chunk, Layer, LayerTiles, TileUse};
pub use citizens::{
    Activity, CitizenChange, CitizenRow, HappinessParts, Population, ScheduleState,
};
pub use clock::{GameTime, TICKS_PER_DAY};
pub use command::{Command, NodeId, RoadKind, ServiceKind, SimSpeed, ZoneKind};
pub use id::{BuildingId, CitizenId, HouseholdId, TripId};
pub use network::{
    Dir, Lane, LaneId, LaneMode, Line, LinkId, Network, Node, NodeControl, Segment, SegmentId,
    SegmentKind,
};
pub use record::{CommandLog, DecodeError, RecordedCommand, ReplayStats, replay};
pub use rng::{Pcg32, RngPurpose, mix_words, rng};
pub use schedule::{BUCKET_COUNT, DepartureQueue, bucket_at, bucket_of};
pub use state_hash::{StateHash, StateHasher, state_hash_of};
pub use terrain::{TerrainTiles, is_water, terrain};
pub use tile::{CHUNK_AREA, CHUNK_SIDE, ChunkCoord, TilePos, TileRect};
pub use units::{LaneOffset, Mm, MmPerTick, Money, Tick, WorldPos};
pub use world::{ApplyOutcome, DenyReason, World};
