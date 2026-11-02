//! chizu のシミュレーションの中核。

pub mod building;
pub mod chunk;
pub mod citizens;
pub mod clock;
pub mod command;
pub mod happiness;
pub mod id;
pub mod jobs;
pub mod movement;
pub mod network;
pub mod record;
pub mod rng;
pub mod round;
pub mod routing;
pub mod schedule;
pub mod service;
pub mod snapshot;
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
pub use movement::{Congestion, Mover, SidewalkPos, Trip, Trips};
pub use network::{
    Dir, KindChange, Lane, LaneId, LaneMode, Line, LinkId, Network, Node, NodeControl, Segment,
    SegmentId, SegmentKind, road_change_cost,
};
pub use record::{CommandLog, DecodeError, RecordedCommand, ReplayStats, SeqGap, replay};
pub use rng::{Pcg32, RngPurpose, mix_words, rng};
pub use schedule::{BUCKET_COUNT, DepartureQueue, bucket_at, bucket_of};
pub use snapshot::{SNAPSHOT_VERSION, SnapshotError};
pub use state_hash::{StateHash, StateHasher, state_hash_of};
pub use terrain::{TerrainTiles, is_water, terrain};
pub use tile::{CHUNK_AREA, CHUNK_SIDE, ChunkCoord, TilePos, TileRect};
pub use units::{LaneOffset, Mm, MmPerTick, Money, Tick, WorldPos};
pub use world::people::MigrationOutlook;
pub use world::{ApplyOutcome, CarSample, CitizenPlaces, DayTraffic, DenyReason, Traffic, World};
