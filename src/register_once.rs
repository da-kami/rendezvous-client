use crate::lib::{
    authenticate_and_multiplex, generate_secret_key_file, load_secret_key_from_file, Behaviour,
    Event,
};
use anyhow::Result;
use libp2p::dns::TokioDnsConfig;
use libp2p::futures::StreamExt;
use libp2p::rendezvous::{Config, Namespace, Rendezvous};
use libp2p::swarm::{AddressScore, SwarmBuilder, SwarmEvent};
use libp2p::tcp::TokioTcpConfig;
use libp2p::{identity, rendezvous, Multiaddr, PeerId, Transport};
use std::fmt::Debug;
use std::path::PathBuf;
use structopt::StructOpt;

mod lib;

#[derive(Debug, StructOpt)]
struct Cli {
    #[structopt(long)]
    rendezvous_peer_id: PeerId,
    #[structopt(long)]
    rendezvous_addr: Multiaddr,
    /// A public facing address is registered with the rendezvous server
    #[structopt(long)]
    external_addr: Multiaddr,
    /// Path to the file that contains the secret key of the rendezvous server's
    /// identity keypair
    #[structopt(long)]
    secret_file: PathBuf,
    /// Set this flag to generate a secret file at the path specified by the
    /// --secret-file argument
    #[structopt(long)]
    generate_secret: bool,
    /// Listen port
    #[structopt(long)]
    port: u16,
    /// Namespace to register in
    #[structopt(long)]
    namespace: String,
    /// Compose the ping behaviour together with the rendezvous behaviour in
    /// case a rendezvous server with Ping is required. This feature will be removed once https://github.com/libp2p/rust-libp2p/issues/2109 is fixed.
    #[structopt(long)]
    ping: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::from_args();

    let secret_key = match cli.generate_secret {
        true => generate_secret_key_file(cli.secret_file).await?,
        false => load_secret_key_from_file(&cli.secret_file).await?,
    };

    let identity = identity::Keypair::Ed25519(secret_key.into());

    let rendezvous_point_address = cli.rendezvous_addr;
    let rendezvous_point = cli.rendezvous_peer_id;

    let tcp_with_dns = TokioDnsConfig::system(TokioTcpConfig::new().nodelay(true)).unwrap();

    let transport = authenticate_and_multiplex(tcp_with_dns.boxed(), &identity).unwrap();

    let rendezvous = Rendezvous::new(identity.clone(), Config::default());

    let peer_id = PeerId::from(identity.public());

    let mut swarm = SwarmBuilder::new(transport, Behaviour::new(rendezvous, cli.ping), peer_id)
        .executor(Box::new(|f| {
            tokio::spawn(f);
        }))
        .build();

    println!("Local peer id: {}", swarm.local_peer_id());

    let _ = swarm.listen_on(format!("/ip4/0.0.0.0/tcp/{}", cli.port).parse().unwrap());

    let _ = swarm.add_external_address(cli.external_addr, AddressScore::Infinite);

    swarm.dial_addr(rendezvous_point_address).unwrap();

    while let Some(event) = swarm.next().await {
        match event {
            SwarmEvent::NewListenAddr(addr) => {
                println!("Listening on {}", addr);
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                cause: Some(error),
                ..
            } if peer_id == rendezvous_point => {
                println!("Lost connection to rendezvous point {}", error);
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                if peer_id == cli.rendezvous_peer_id {
                    swarm.behaviour_mut().rendezvous.register(
                        Namespace::new(cli.namespace.clone())?,
                        rendezvous_point,
                        None,
                    );
                }
            }
            SwarmEvent::Behaviour(Event::Rendezvous(rendezvous::Event::Registered {
                namespace,
                ttl,
                rendezvous_node,
            })) => {
                println!(
                    "Registered for namespace '{}' at rendezvous point {} for the next {} seconds",
                    namespace, rendezvous_node, ttl
                );
                return Ok(());
            }
            SwarmEvent::Behaviour(Event::Rendezvous(rendezvous::Event::RegisterFailed(error))) => {
                println!("Failed to register {:?}", error);
            }
            other => {
                println!("Unhandled {:?}", other);
            }
        }
    }

    Ok(())
}
