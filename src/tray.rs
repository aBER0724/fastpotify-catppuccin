//! A status-notifier tray item (Linux), so closing the window can leave the
//! music playing.
//!
//! The tray runs on its own thread and exchanges bounded messages with the
//! interface, exactly like MPRIS: a missing or broken status-notifier host
//! must never take audio or the window down with it. When no host is
//! present, spawning fails and the app simply quits on close as before.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use ksni::blocking::TrayMethods;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    Show,
    ShowHide,
    PlayPause,
    Next,
    Previous,
    Quit,
}

struct FastTray {
    commands: Sender<TrayCommand>,
    wake: Arc<dyn Fn() + Send + Sync>,
    playing: bool,
}

impl FastTray {
    fn send(&self, command: TrayCommand) {
        if self.commands.send(command).is_ok() {
            (self.wake)();
        }
    }
}

impl ksni::Tray for FastTray {
    fn id(&self) -> String {
        "fastpotify".into()
    }

    fn title(&self) -> String {
        "Fastpotify".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let size = 64usize;
        let rgba = crate::util::app_icon_rgba(size);
        // ksni wants ARGB32 in network byte order.
        let mut data = Vec::with_capacity(rgba.len());
        let (pixels, _) = rgba.as_chunks::<4>();
        for [r, g, b, a] in pixels {
            data.extend_from_slice(&[*a, *r, *g, *b]);
        }
        vec![ksni::Icon {
            width: size as i32,
            height: size as i32,
            data,
        }]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(TrayCommand::ShowHide);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            StandardItem {
                label: "Show or hide Fastpotify".into(),
                activate: Box::new(|tray: &mut Self| tray.send(TrayCommand::ShowHide)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: if self.playing {
                    "Pause".into()
                } else {
                    "Play".into()
                },
                activate: Box::new(|tray: &mut Self| tray.send(TrayCommand::PlayPause)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Next".into(),
                activate: Box::new(|tray: &mut Self| tray.send(TrayCommand::Next)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Previous".into(),
                activate: Box::new(|tray: &mut Self| tray.send(TrayCommand::Previous)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|tray: &mut Self| tray.send(TrayCommand::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub struct TrayService {
    handle: ksni::blocking::Handle<FastTray>,
    commands: Receiver<TrayCommand>,
    playing: bool,
}

impl TrayService {
    /// Registers the tray item. `None` when no status-notifier host exists.
    pub fn spawn(wake: impl Fn() + Send + Sync + 'static) -> Option<Self> {
        let (sender, commands) = std::sync::mpsc::channel();
        let tray = FastTray {
            commands: sender,
            wake: Arc::new(wake),
            playing: false,
        };
        // Flatpak allows talking to the watcher, but not owning ksni's
        // generated StatusNotifierItem name. Register the unique connection
        // name instead, as ksni requires for sandboxed applications.
        let flatpak = std::path::Path::new("/.flatpak-info").exists()
            || std::env::var_os("FLATPAK_ID").is_some();
        match tray.disable_dbus_name(flatpak).spawn() {
            Ok(handle) => Some(Self {
                handle,
                commands,
                playing: false,
            }),
            Err(error) => {
                log::info!("no system tray available: {error}");
                None
            }
        }
    }

    pub fn drain_commands(&self) -> Vec<TrayCommand> {
        self.commands.try_iter().collect()
    }

    /// Keeps the menu's Play/Pause label matching reality.
    pub fn set_playing(&mut self, playing: bool) {
        if self.playing != playing {
            self.playing = playing;
            self.handle.update(|tray| tray.playing = playing);
        }
    }

    /// Nothing to do: the item lives on its own thread from the start.
    pub fn attach(&mut self) {}

    /// Nothing to do either; see `attach`.
    pub fn hidden(&mut self) {}
}

/// Waits while the app lives in the tray without a window. Linux has
/// nothing to pump here: the tray and MPRIS run on their own threads.
pub fn idle(duration: Duration) {
    std::thread::sleep(duration);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::process::{Command, Stdio};

    struct Watcher(Sender<String>);

    #[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
    impl Watcher {
        fn register_status_notifier_item(&self, service: &str) {
            self.0.send(service.to_string()).expect("registration");
        }

        #[zbus(property)]
        fn is_status_notifier_host_registered(&self) -> bool {
            true
        }
    }

    /// Exercise the actual tray on a private bus that permits the watcher
    /// name but denies every application-owned name, like Flatpak's proxy.
    /// A subprocess keeps the test's bus and sandbox environment isolated.
    #[test]
    fn flatpak_tray_registers_without_owning_a_name() {
        const CHILD: &str = "FASTPOTIFY_TRAY_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let (registered, registrations) = std::sync::mpsc::channel();
            let server = zbus::blocking::connection::Builder::session()
                .unwrap()
                .name("org.kde.StatusNotifierWatcher")
                .unwrap()
                .serve_at("/StatusNotifierWatcher", Watcher(registered))
                .unwrap()
                .build()
                .unwrap();
            let (commands, _) = std::sync::mpsc::channel();
            assert!(
                FastTray {
                    commands,
                    wake: Arc::new(|| {}),
                    playing: false
                }
                .spawn()
                .is_err(),
                "the bus must reject the previous registration method"
            );
            let tray = TrayService::spawn(|| {}).expect("sandbox registration succeeds");
            let name = registrations.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(name.starts_with(':'), "register the unique connection name");
            server
                .call_method(
                    Some(name.as_str()),
                    "/StatusNotifierItem",
                    Some("org.kde.StatusNotifierItem"),
                    "Activate",
                    &(0_i32, 0_i32),
                )
                .unwrap();
            assert_eq!(tray.drain_commands(), vec![TrayCommand::ShowHide]);
            server.close().unwrap();
            assert!(
                TrayService::spawn(|| {}).is_none(),
                "no watcher still means no tray"
            );
            return;
        }

        let root = std::env::temp_dir().join(format!("fastpotify-tray-bus-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let config = root.join("bus.conf");
        std::fs::write(&config, r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen>
<policy context="default"><allow user="*"/><allow send_destination="*"/>
<allow receive_sender="*"/><deny own="*"/>
<allow own="org.kde.StatusNotifierWatcher"/></policy></busconfig>"#).unwrap();
        let mut bus = match Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .spawn()
        {
            Ok(bus) => bus,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("dbus-daemon is unavailable; private-bus tray test skipped");
                let _ = std::fs::remove_dir_all(root);
                return;
            }
            Err(error) => panic!("private bus: {error}"),
        };
        let mut address = String::new();
        std::io::BufReader::new(bus.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tray::tests::flatpak_tray_registers_without_owning_a_name",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("FLATPAK_ID", "rocks.fastpotify.Fastpotify")
            .env("DBUS_SESSION_BUS_ADDRESS", address.trim())
            .status();
        let _ = bus.kill();
        let _ = bus.wait();
        let _ = std::fs::remove_dir_all(root);
        assert!(
            result.unwrap().success(),
            "tray registration and activation must work"
        );
    }
}
