pub mod cluster;
pub mod flags;
pub mod model;
pub mod planner;
pub mod profile;
pub mod process;
pub mod stats;

#[cfg(test)]
mod tests {
    #[test]
    fn workspace_builds() {
        assert_eq!(2 + 2, 4);
    }
}
