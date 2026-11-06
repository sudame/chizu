//! chizu のシミュレーションの中核。

pub mod building;
pub mod car_place;
pub mod chunk;
pub mod citizens;
pub mod clock;
pub mod command;
pub mod happiness;
pub mod id;
pub mod jobs;
pub mod movement;
pub mod network;
pub mod rail;
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

pub use car_place::{CarPlace, CarPlaces};
pub use chunk::{Chunk, Layer, LayerTiles, TileUse};
pub use citizens::{
    Activity, CitizenChange, CitizenRow, HappinessParts, Population, ScheduleState,
};
pub use clock::{GameTime, TICKS_PER_DAY};
pub use command::{
    Command, Density, Heading, LOW_DENSITY_MAX_LEVEL, NodeId, RoadKind, ServiceKind, SimSpeed,
    Zone, ZoneKind,
};
pub use id::{BuildingId, CitizenId, HouseholdId, TripId};
pub use movement::{
    CarPath, Congestion, CurbWait, DailyLotMisses, HouseholdCar, LotId, LotMisses, Mover, Noted,
    ParkingTally, Rescues, SHORTAGE_LOTS_FULL, SharedLot, SidewalkPos, Trip, Trips, WalkAhead,
    WalkPath,
};
pub use network::{
    Dir, KindChange, Lane, LaneId, LaneMode, Line, LinkId, Network, Node, NodeControl, Segment,
    SegmentId, SegmentKind, TILE_MM, road_change_cost,
};
pub use rail::{
    RAIL_BLOCK_MAX_TILES, Rail, RailNode, RailNodeId, Station, StationId, Track, TrackId, rail_cost,
};
pub use record::{CommandLog, DecodeError, RecordedCommand, ReplayStats, SeqGap, replay};
pub use rng::{Pcg32, RngPurpose, mix_words, rng};
pub use schedule::{BUCKET_COUNT, DepartureQueue, bucket_at, bucket_of};
pub use snapshot::{SNAPSHOT_VERSION, SnapshotError};
pub use state_hash::{StateHash, StateHasher, state_hash_of};
pub use terrain::{TerrainTiles, is_water, terrain};
pub use tile::{CHUNK_AREA, CHUNK_SIDE, ChunkCoord, TilePos, TileRect};
pub use units::{LaneOffset, Mm, MmPerTick, Money, Tick, WorldPos};
pub use world::carless::{CARLESS_AREA_SIDE, Carless, CarlessArea, CarlessReason, carless_areas};
pub use world::people::MigrationOutlook;
pub use world::{
    ApplyOutcome, CarSample, CitizenPlaces, CommuteDistances, CommuteTimes, DayTraffic, DenyReason,
    Traffic, World,
};
