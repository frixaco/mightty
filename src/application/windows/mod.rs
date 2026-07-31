//! Windows application lifecycle services.

mod instance;

pub use instance::{InstanceClaim, PrimaryInstance, claim_instance, send_activation};
