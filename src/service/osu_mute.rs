use std::{sync::Arc, time::Duration};

use log::debug;
use tokio::{
    spawn,
    sync::{Notify, RwLock},
    task::JoinHandle,
    time::sleep,
};

use crate::setlock::SetLock;

use super::{discord::DiscordService, Priority, Service, ServiceInfo, ServiceManager};

pub struct OsuMuteService {
    info: ServiceInfo,
    discord_service: SetLock<Arc<RwLock<DiscordService>>>,
    watchdog_notify: Arc<RwLock<SetLock<Notify>>>,
    watchdog: SetLock<JoinHandle<()>>,
}

impl OsuMuteService {
    pub fn new() -> Self {
        Self {
            info: ServiceInfo::new("lum_builtin_osu_mute", "OsuMute", Priority::Optional),
            discord_service: SetLock::new(),
            watchdog_notify: Arc::new(RwLock::new(SetLock::new())),
            watchdog: SetLock::new(),
        }
    }
}

impl Default for OsuMuteService {
    fn default() -> Self {
        Self::new()
    }
}

impl Service for OsuMuteService {
    fn info(&self) -> &ServiceInfo {
        &self.info
    }

    fn start(
        &mut self,
        service_manager: Arc<ServiceManager>,
    ) -> super::PinnedBoxedFutureResult<'_, ()> {
        Box::pin(async move {
            match service_manager.get_service::<DiscordService>().await {
                Some(discord_service) => {
                    if !discord_service.read().await.is_available().await {
                        return Err("DiscordService is not available!".into());
                    }

                    if let Err(error) = self.discord_service.set(discord_service.clone()) {
                        return Err(
                            format!("Error setting DiscordService SetLock: {}", error).into()
                        );
                    }
                }
                None => return Err("DiscordService not found!".into()),
            }

            if let Err(error) = self.watchdog_notify.write().await.set(Notify::new()) {
                return Err(format!("Error setting Notify SetLock: {}", error).into());
            }

            if let Err(error) = self.watchdog.set(spawn(watchdog())) {
                return Err(format!("Error setting Watchdog JoinHandle SetLock: {}", error).into());
            }

            Ok(())
        })
    }

    fn stop(&mut self) -> super::PinnedBoxedFutureResult<'_, ()> {
        Box::pin(async move { Ok(()) })
    }
}

async fn watchdog() {
    loop {
        debug!("Watchdog ticked!");
        sleep(Duration::from_secs(5)).await;
    }
}
