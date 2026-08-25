use bifrost_api::AppStatus;

#[derive(Debug, Default)]
pub struct Application {
    status: AppStatus,
}

impl Application {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status(&self) -> AppStatus {
        self.status.clone()
    }
}
