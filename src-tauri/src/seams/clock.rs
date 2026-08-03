pub trait Clock: Send + Sync {
    fn monotonic_millis(&self) -> u128;
    fn unix_epoch_nanos(&self) -> u128;
}
