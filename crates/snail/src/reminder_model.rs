//! App-lifetime calendar reminder service (plan.md E13.7).
//!
//! This entity is not attached to a window. Closing the last window does not discard its timer;
//! durable deliveries are reloaded after sleep or process restart.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;

use snail_core::store::Store;
use snail_services::reminders;

pub struct ReminderHub {
    _loop: Task<()>,
}

struct Global(Entity<ReminderHub>);
impl gpui_kit::Global for Global {}

impl ReminderHub {
    pub fn install(store: Arc<Store>, cx: &mut App) -> Entity<Self> {
        let hub = cx.new(|cx| Self::new(store, cx));
        cx.set_global(Global(hub.clone()));
        hub
    }

    fn new(store: Arc<Store>, cx: &mut Context<Self>) -> Self {
        let run = cx.spawn(async move |_this, cx| {
            loop {
                let worker_store = store.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let now = chrono::Utc::now().timestamp();
                        reminders::tick(&worker_store, now).map(|pass| (worker_store, pass, now))
                    })
                    .await;
                match result {
                    Ok((worker_store, pass, now)) => {
                        let due = pass.due;
                        let posted = due.clone();
                        cx.update(|cx| {
                            for reminder in &posted {
                                cx.show_system_notification(SystemNotification {
                                    tag: format!(
                                        "calendar-reminder:{}:{}",
                                        reminder.reminder_id, reminder.occurrence_start
                                    )
                                    .into(),
                                    title: reminder.title.clone().into(),
                                    body: if reminder.due_utc < now {
                                        "Calendar reminder · delivered after wake".into()
                                    } else {
                                        "Calendar reminder".into()
                                    },
                                    actions: Vec::new(),
                                });
                            }
                        });
                        cx.background_executor()
                            .spawn(async move {
                                if let Err(error) = reminders::acknowledge(&worker_store, &due, now)
                                {
                                    log::warn!(
                                        "could not acknowledge calendar reminder: {error:#}"
                                    );
                                }
                            })
                            .detach();
                    }
                    Err(error) => log::warn!("calendar reminder pass failed: {error:#}"),
                }
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
            }
        });
        Self { _loop: run }
    }
}
