//! End-to-end SSH tests against an in-process `russh` server.
//!
//! These exercise the real transport: TCP, key exchange, password authentication, PTY
//! request, shell request, data in both directions and resize.

use std::sync::Arc;
use std::time::{Duration, Instant};

use russh::keys::PrivateKey;
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId, Pty};
use wassh_core::SessionStatus;
use wassh_ssh::{
    HostKeyFingerprint, HostKeyPolicy, KnownHosts, SshAuth, SshEvent, SshOptions, SshSession,
};

#[derive(Clone)]
struct EchoServer;

impl server::Handler for EchoServer {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if password == "secret" {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let _ = session.channel_success(channel);
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let _ = session.channel_success(channel);
        session.data(channel, b"READY\r\n".to_vec())?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let reply = format!("echo:{}", String::from_utf8_lossy(data));
        session.data(channel, reply.into_bytes())?;
        Ok(())
    }
}

/// Throwaway ed25519 host key used only by these tests.
const TEST_HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACC3YJilmtTL1Yzxj1k9A91pbvonkDdGrD49/iE9WfkUBAAAAJAA4kMiAOJD\nIgAAAAtzc2gtZWQyNTUxOQAAACC3YJilmtTL1Yzxj1k9A91pbvonkDdGrD49/iE9WfkUBA\nAAAECoQUbFK7LflE6UdHX/68zm8oBEXxSmq7niKCwA/yVCNLdgmKWa1MvVjPGPWT0D3Wlu\n+ieQN0asPj3+IT1Z+RQEAAAACndhc3NoLXRlc3QBAgM=\n-----END OPENSSH PRIVATE KEY-----\n";

async fn spawn_server() -> (String, u16) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let config = Arc::new(server::Config {
            keys: vec![
                PrivateKey::from_openssh(TEST_HOST_KEY).expect("parse the test host key"),
            ],
            ..Default::default()
        });
        let _ = server::run_stream(config, stream, EchoServer).await;
    });
    (address.ip().to_string(), address.port())
}

fn options(host: String, port: u16, auth: SshAuth) -> SshOptions {
    let mut options = SshOptions::new(host, "tester", auth);
    options.port = port;
    options.connect_timeout = Duration::from_secs(5);
    options
}

async fn wait_for(
    session: &mut SshSession,
    deadline: Duration,
    mut done: impl FnMut(&str, &mut Vec<SshEvent>, bool) -> bool,
) -> (String, Vec<SshEvent>) {
    let start = Instant::now();
    let mut text = String::new();
    let mut seen = Vec::new();
    let mut connected = false;
    while start.elapsed() < deadline {
        for event in session.events() {
            match &event {
                SshEvent::Status { status, .. } if *status == SessionStatus::Connected => {
                    connected = true;
                }
                SshEvent::Data(bytes) => text.push_str(&String::from_utf8_lossy(bytes)),
                SshEvent::Error(error) => panic!("ssh error: {error}"),
                _ => {}
            }
            seen.push(event);
        }
        if done(&text, &mut seen, connected) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (text, seen)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connects_authenticates_and_echoes() {
    let (host, port) = spawn_server().await;
    let mut session = SshSession::connect(options(host, port, SshAuth::Password("secret".to_owned())))
        .expect("spawn session");

    let (text, seen) = wait_for(&mut session, Duration::from_secs(10), |text, _, connected| {
        connected && text.contains("READY")
    })
    .await;

    assert!(
        seen.iter().any(|event| matches!(
            event,
            SshEvent::Status {
                status: SessionStatus::Connected,
                ..
            }
        )),
        "connected status missing: {seen:?}"
    );
    assert!(text.contains("READY"), "banner missing: {text:?}");

    session.write(b"ping");
    let (text, _) = wait_for(&mut session, Duration::from_secs(5), |text, _, _| {
        text.contains("echo:ping")
    })
    .await;
    assert!(text.contains("echo:ping"), "echo missing: {text:?}");

    // Resize must not tear the session down.
    session.resize(132, 43);
    session.write(b"after-resize");
    let (text, _) = wait_for(&mut session, Duration::from_secs(5), |text, _, _| {
        text.contains("echo:after-resize")
    })
    .await;
    assert!(text.contains("echo:after-resize"), "echo after resize: {text:?}");

    session.disconnect();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sends_x11_request_with_display_details() {
    use std::sync::Mutex;
    use wassh_ssh::X11Config;

    #[derive(Clone, Default)]
    struct X11Server {
        requests: Arc<Mutex<Vec<(String, String, u32)>>>,
    }

    impl server::Handler for X11Server {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            _user: &str,
            password: &str,
        ) -> Result<Auth, Self::Error> {
            Ok(if password == "secret" {
                Auth::Accept
            } else {
                Auth::reject()
            })
        }

        async fn channel_open_session(
            &mut self,
            _channel: Channel<Msg>,
            reply: ChannelOpenHandle,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn pty_request(
            &mut self,
            channel: ChannelId,
            _term: &str,
            _cols: u32,
            _rows: u32,
            _pw: u32,
            _ph: u32,
            _modes: &[(Pty, u32)],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            let _ = session.channel_success(channel);
            Ok(())
        }

        async fn shell_request(
            &mut self,
            channel: ChannelId,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            let _ = session.channel_success(channel);
            Ok(())
        }

        async fn x11_request(
            &mut self,
            channel: ChannelId,
            _single_connection: bool,
            protocol: &str,
            cookie: &str,
            screen: u32,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            let _ = session.channel_success(channel);
            self.requests
                .lock()
                .expect("requests lock")
                .push((protocol.to_owned(), cookie.to_owned(), screen));
            Ok(())
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let server_requests = requests.clone();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let config = Arc::new(server::Config {
            keys: vec![
                PrivateKey::from_openssh(TEST_HOST_KEY).expect("parse the test host key"),
            ],
            ..Default::default()
        });
        let _ = server::run_stream(
            config,
            stream,
            X11Server {
                requests: server_requests,
            },
        )
        .await;
    });

    let mut options = options(
        address.ip().to_string(),
        address.port(),
        SshAuth::Password("secret".to_owned()),
    );
    options.x11 = Some(X11Config {
        display_number: 0,
        screen: 2,
        protocol: "MIT-MAGIC-COOKIE-1".to_owned(),
        cookie_hex: "00112233".to_owned(),
    });

    let session = SshSession::connect(options).expect("spawn session");

    let start = Instant::now();
    let mut observed = None;
    while start.elapsed() < Duration::from_secs(10) {
        let _ = session.events();
        if let Some(first) = requests.lock().expect("requests lock").first().cloned() {
            observed = Some(first);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(
        observed,
        Some(("MIT-MAGIC-COOKIE-1".to_owned(), "00112233".to_owned(), 2)),
        "x11-req was not sent with the expected protocol, cookie and screen"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_a_bad_password() {
    let (host, port) = spawn_server().await;
    let session = SshSession::connect(options(
        host,
        port,
        SshAuth::Password("wrong".to_owned()),
    ))
    .expect("spawn session");

    let start = Instant::now();
    let mut failures = Vec::new();
    while start.elapsed() < Duration::from_secs(10) {
        for event in session.events() {
            // Rejected credentials arrive as their own event so the UI can re-prompt.
            if let SshEvent::AuthFailed { detail } = event {
                failures.push(detail);
            }
        }
        if !failures.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(
        failures.iter().any(|error| error.contains("authentication failed")),
        "expected an authentication failure, got {failures:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_keyboard_interactive_prompts() {
    use std::borrow::Cow;

    #[derive(Clone)]
    struct KbdServer;

    impl server::Handler for KbdServer {
        type Error = russh::Error;

        async fn auth_keyboard_interactive<'a>(
            &'a mut self,
            _user: &str,
            _submethods: &str,
            response: Option<server::Response<'a>>,
        ) -> Result<Auth, Self::Error> {
            match response {
                None => Ok(Auth::Partial {
                    name: Cow::Borrowed("verification"),
                    instructions: Cow::Borrowed("Enter the code"),
                    prompts: Cow::Owned(vec![(Cow::Borrowed("Code: "), false)]),
                }),
                Some(mut answers) => {
                    let code = answers
                        .next()
                        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
                        .unwrap_or_default();
                    Ok(if code == "123456" {
                        Auth::Accept
                    } else {
                        Auth::reject()
                    })
                }
            }
        }

        async fn channel_open_session(
            &mut self,
            _channel: Channel<Msg>,
            reply: ChannelOpenHandle,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn pty_request(
            &mut self,
            channel: ChannelId,
            _term: &str,
            _cols: u32,
            _rows: u32,
            _pw: u32,
            _ph: u32,
            _modes: &[(Pty, u32)],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            let _ = session.channel_success(channel);
            Ok(())
        }

        async fn shell_request(
            &mut self,
            channel: ChannelId,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            let _ = session.channel_success(channel);
            session.data(channel, b"READY\r\n".to_vec())?;
            Ok(())
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let config = Arc::new(server::Config {
            keys: vec![
                PrivateKey::from_openssh(TEST_HOST_KEY).expect("parse the test host key"),
            ],
            ..Default::default()
        });
        let _ = server::run_stream(config, stream, KbdServer).await;
    });

    let mut options = options(
        address.ip().to_string(),
        address.port(),
        SshAuth::KeyboardInteractive,
    );
    options.host_key_policy = HostKeyPolicy::AcceptAny;
    let mut session = SshSession::connect(options).expect("spawn session");

    let mut fields = None;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        for event in session.events() {
            if let SshEvent::AuthPrompt { prompts, .. } = event {
                fields = Some(prompts);
            }
        }
        if fields.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let fields = fields.expect("auth prompt");
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].label, "Code: ");
    assert!(!fields[0].echo, "one-time codes must be masked");

    session.respond_auth(vec!["123456".to_owned()]);
    let (text, _) = wait_for(&mut session, Duration::from_secs(10), |text, _, connected| {
        connected && text.contains("READY")
    })
    .await;
    assert!(text.contains("READY"), "authentication did not complete: {text:?}");
}

async fn wait_for_host_key_prompt(session: &SshSession) -> Option<(String, bool)> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        for event in session.events() {
            if let SshEvent::HostKeyPrompt {
                fingerprint,
                mismatch,
                ..
            } = event
            {
                return Some((fingerprint, mismatch));
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asks_before_trusting_an_unknown_host_key() {
    let (host, port) = spawn_server().await;
    let known = Arc::new(KnownHosts::in_memory());
    let mut options = options(host.clone(), port, SshAuth::Password("secret".to_owned()));
    options.host_key_policy = HostKeyPolicy::Ask(known.clone());
    let mut session = SshSession::connect(options).expect("spawn session");

    let (fingerprint, mismatch) = wait_for_host_key_prompt(&session)
        .await
        .expect("host-key prompt");
    assert!(!mismatch, "a new host must not be flagged as a mismatch");
    assert!(fingerprint.starts_with("SHA256:"));

    session.respond_host_key(true);
    let (text, _) = wait_for(&mut session, Duration::from_secs(10), |text, _, connected| {
        connected && text.contains("READY")
    })
    .await;
    assert!(text.contains("READY"), "never reached a shell: {text:?}");
    assert_eq!(known.get(&host, port).as_deref(), Some(fingerprint.as_str()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_a_changed_host_key_without_overwriting_it() {
    const WRONG: &str = "SHA256:definitely-not-the-server-key";
    let (host, port) = spawn_server().await;
    let known = Arc::new(KnownHosts::in_memory());
    known
        .remember(HostKeyFingerprint::new(host.clone(), port, WRONG))
        .unwrap();

    let mut options = options(host.clone(), port, SshAuth::Password("secret".to_owned()));
    options.host_key_policy = HostKeyPolicy::Ask(known.clone());
    let session = SshSession::connect(options).expect("spawn session");

    let (_, mismatch) = wait_for_host_key_prompt(&session)
        .await
        .expect("host-key prompt");
    assert!(mismatch, "a changed key must be flagged");

    session.respond_host_key(false);

    let start = Instant::now();
    let mut failed = false;
    while start.elapsed() < Duration::from_secs(10) {
        for event in session.events() {
            if matches!(event, SshEvent::Error(_) | SshEvent::Closed { .. }) {
                failed = true;
            }
        }
        if failed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(failed, "rejecting the key must abort the connection");
    assert_eq!(known.get(&host, port).as_deref(), Some(WRONG));
}
