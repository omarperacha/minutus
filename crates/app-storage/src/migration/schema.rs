mod v10_v17;
mod v1_v9;

pub(super) use v1_v9::{
    MIGRATION_1, MIGRATION_2, MIGRATION_3, MIGRATION_4, MIGRATION_5, MIGRATION_6, MIGRATION_7,
    MIGRATION_8, MIGRATION_9,
};
pub(super) use v10_v17::{
    MIGRATION_10, MIGRATION_11, MIGRATION_12, MIGRATION_13, MIGRATION_14, MIGRATION_15,
    MIGRATION_16, MIGRATION_17,
};

mod v18;
pub(super) use v18::MIGRATION_18;

mod v20;
pub(super) use v20::MIGRATION_20;

mod v19;
pub(super) use v19::MIGRATION_19;

mod v21;
pub(super) use v21::MIGRATION_21;

mod v22;
pub(super) use v22::MIGRATION_22;

mod v23;
pub(super) use v23::MIGRATION_23;

mod v24;
pub(super) use v24::MIGRATION_24;
