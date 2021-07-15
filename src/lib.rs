use anyhow::{Context, Result};
use futures::{AsyncRead, AsyncWrite};
use libp2p::core::identity;
use libp2p::core::identity::ed25519::{Keypair, SecretKey};
use libp2p::core::muxing::StreamMuxerBox;
use libp2p::core::transport::Boxed;
use libp2p::core::upgrade::{SelectUpgrade, Version};
use libp2p::mplex::MplexConfig;
use libp2p::noise::{NoiseConfig, X25519Spec};
use libp2p::ping::{Ping, PingConfig, PingEvent};
use libp2p::rendezvous::Rendezvous;
use libp2p::yamux::YamuxConfig;
use libp2p::{noise, rendezvous, NetworkBehaviour, PeerId, Transport};
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs;
use tokio::fs::{DirBuilder, OpenOptions};
use tokio::io::AsyncWriteExt;

#[derive(Debug)]
pub enum Event {
    Rendezvous(rendezvous::Event),
    Ping(PingEvent),
}

impl From<rendezvous::Event> for Event {
    fn from(event: rendezvous::Event) -> Self {
        Event::Rendezvous(event)
    }
}

impl From<PingEvent> for Event {
    fn from(event: PingEvent) -> Self {
        Event::Ping(event)
    }
}

#[derive(NetworkBehaviour)]
#[behaviour(event_process = false)]
#[behaviour(out_event = "Event")]
pub struct Behaviour {
    ping: Ping,
    pub rendezvous: Rendezvous,
}

impl Behaviour {
    #[allow(dead_code)]
    pub fn new(rendezvous: Rendezvous) -> Self {
        Self {
            // TODO: Remove Ping behaviour once https://github.com/libp2p/rust-libp2p/issues/2109 is fixed
            // interval for sending Ping set to 24 hours
            ping: Ping::new(
                PingConfig::new()
                    .with_keep_alive(false)
                    .with_interval(Duration::from_secs(86_400)),
            ),
            rendezvous,
        }
    }
}

#[allow(dead_code)]
pub async fn load_secret_key_from_file(path: impl AsRef<Path> + Debug) -> Result<SecretKey> {
    let bytes = fs::read(&path)
        .await
        .with_context(|| format!("No secret file at {:?}", path))?;
    let secret_key = SecretKey::from_bytes(bytes)?;
    Ok(secret_key)
}

#[allow(dead_code)]
pub async fn generate_secret_key_file(path: PathBuf) -> Result<SecretKey> {
    if let Some(parent) = path.parent() {
        DirBuilder::new()
            .recursive(true)
            .create(parent)
            .await
            .with_context(|| format!("Could not create directory for secret file: {:?}", parent))?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
        .with_context(|| format!("Could not generate secret file at {:?}", &path))?;

    let keypair = Keypair::generate();
    let secret_key = SecretKey::from(keypair);

    file.write_all(secret_key.as_ref()).await?;

    Ok(secret_key)
}

pub fn authenticate_and_multiplex<T>(
    transport: Boxed<T>,
    identity: &identity::Keypair,
) -> Result<Boxed<(PeerId, StreamMuxerBox)>>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let auth_upgrade = {
        let noise_identity = noise::Keypair::<X25519Spec>::new().into_authentic(identity)?;
        NoiseConfig::xx(noise_identity).into_authenticated()
    };
    let multiplex_upgrade = SelectUpgrade::new(YamuxConfig::default(), MplexConfig::new());

    let transport = transport
        .upgrade(Version::V1)
        .authenticate(auth_upgrade)
        .multiplex(multiplex_upgrade)
        .timeout(Duration::from_secs(20))
        .map(|(peer, muxer), _| (peer, StreamMuxerBox::new(muxer)))
        .boxed();

    Ok(transport)
}
