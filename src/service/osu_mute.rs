use std::{
    collections::HashMap,
    error::Error,
    fmt::{self, Display, Formatter},
    sync::Arc,
    time::Duration,
};

use log::{error, info};
use serenity::{
    all::{GuildChannel, Member},
    FutureExt,
};
use tokio::{
    spawn,
    sync::{Notify, RwLock},
    task::JoinHandle,
    time::sleep,
};

use crate::setlock::SetLock;

use super::{
    discord::DiscordService, PinnedBoxedFutureResult, Priority, Service, ServiceInfo,
    ServiceManager, Status,
};

pub struct OsuMuteService {
    info: ServiceInfo,
    discord_service: SetLock<Arc<RwLock<DiscordService>>>,
    task_notify: Arc<RwLock<SetLock<Notify>>>,
    task: SetLock<JoinHandle<()>>,
    pub muted_users: RwLock<HashMap<GuildChannel, Member>>,
}

impl OsuMuteService {
    pub fn new() -> Self {
        Self {
            info: ServiceInfo::new("lum_builtin_osu_mute", "osu! Mute", Priority::Optional),
            discord_service: SetLock::new(),
            task_notify: Arc::new(RwLock::new(SetLock::new())),
            task: SetLock::new(),
            muted_users: RwLock::new(HashMap::new()),
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

    fn start(&mut self, service_manager: Arc<ServiceManager>) -> PinnedBoxedFutureResult<'_, ()> {
        Box::pin(async move {
            match service_manager.get_service::<DiscordService>().await {
                Some(discord_service) => {
                    if !discord_service.read().await.is_available().await {
                        return Err("DiscordService is not available!".into());
                    }

                    let result = self.discord_service.set(discord_service.clone());
                    if let Err(error) = result {
                        return Err(
                            format!("Error setting DiscordService SetLock: {}", error).into()
                        );
                    }
                }
                None => return Err("DiscordService not found!".into()),
            }

            let result = self.task_notify.write().await.set(Notify::new());
            if let Err(error) = result {
                return Err(format!("Error setting Notify SetLock: {}", error).into());
            }

            let task = task(Arc::clone(&service_manager));
            let task_with_watchdog = task
                .then(|result| async move { watchdog(Arc::clone(&service_manager), result).await });

            let task_handle = spawn(task_with_watchdog);
            let result = self.task.set(task_handle);
            if let Err(error) = result {
                return Err(format!("Error setting Watchdog JoinHandle SetLock: {}", error).into());
            }

            Ok(())
        })
    }

    fn stop(&mut self) -> PinnedBoxedFutureResult<'_, ()> {
        Box::pin(async move {
            self.task.unwrap().abort();
            Ok(())
        })
    }
}

#[derive(Debug)]
enum TaskError {
    DiscordServiceNotFound,
    DiscordServiceNotAvailable(String),
}

impl Display for TaskError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::DiscordServiceNotFound => write!(f, "Discord service not found!"),
            Self::DiscordServiceNotAvailable(status) => write!(
                f,
                "Discord service expected to be available, but it was {}",
                status
            ),
        }
    }
}

impl Error for TaskError {}

async fn task(service_manager: Arc<ServiceManager>) -> Result<(), TaskError> {
    let osu_mute_service = match service_manager.get_service::<OsuMuteService>().await {
        Some(osu_mute_service) => osu_mute_service,
        None => return Err(TaskError::DiscordServiceNotFound),
    };

    loop {
        //TODO: When Rust allows async trait methods to be object-safe, refactor this to use service.is_available()
        let osu_mute_service_lock = osu_mute_service.read().await;
        let muted_users = osu_mute_service_lock.muted_users.read().await;

        let are_users_muted = muted_users.is_empty();
        drop(muted_users);

        if !are_users_muted {
            osu_mute_service_lock
                .task_notify
                .read()
                .await
                .unwrap()
                .notified()
                .await;
        }

        let discord_service = Arc::clone(osu_mute_service_lock.discord_service.unwrap());
        let discord_service_lock = discord_service.read().await;

        let discord_service_status = discord_service_lock.info().status.read().await;
        if !matches!(*discord_service_status, Status::Started) {
            return Err(TaskError::DiscordServiceNotAvailable(
                discord_service_status.to_string(),
            ));
        }
        drop(discord_service_status);

        //TODO: Add logic to mute users
        sleep(Duration::from_secs(1)).await;
        info!("Tick");
    }
}

async fn watchdog(service_manager: Arc<ServiceManager>, result: Result<(), TaskError>) {
    let osu_mute_service = match service_manager.get_service::<OsuMuteService>().await {
        Some(osu_mute_service) => osu_mute_service,
        None => panic!("Watchdog failed to get OsuMuteService"),
    };

    let osu_mute_service_lock = osu_mute_service.read().await;
    let mut osu_mute_service_status = osu_mute_service_lock.info().status.write().await;

    match result {
        Ok(()) => {
            *osu_mute_service_status =
                Status::RuntimeError("The background task has stopped unexpectedly.".into());
        }
        Err(error) => {
            *osu_mute_service_status = Status::RuntimeError(
                format!("The background task has encountered an error: {}", error).into(),
            );
        }
    }

    error!(
        "Watchdog triggered for service {}. {}",
        osu_mute_service_lock.info().id,
        osu_mute_service_status
    );
}
