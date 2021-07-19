use crate::lib::{authenticate_and_multiplex, generate_secret_key_file, load_secret_key_from_file};
use anyhow::Result;
use futures::FutureExt;
use libp2p::core::connection::ConnectionId;
use libp2p::dns::TokioDnsConfig;
use libp2p::futures::StreamExt;
use libp2p::rendezvous::Namespace;
use libp2p::swarm::{
    AddressScore, DialPeerCondition, IntoProtocolsHandler, NetworkBehaviour,
    NetworkBehaviourAction, PollParameters, ProtocolsHandler, SwarmBuilder, SwarmEvent,
};
use libp2p::tcp::TokioTcpConfig;
use libp2p::{identity, rendezvous, Multiaddr, NetworkBehaviour, PeerId, Transport};
use rendezvous_client::Event;
use std::fmt::Debug;
use std::path::PathBuf;
use std::task::Poll;
use std::time::Duration;
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
    let rendezvous_point_peer_id = cli.rendezvous_peer_id;

    let tcp_with_dns = TokioDnsConfig::system(TokioTcpConfig::new().nodelay(true)).unwrap();

    let transport = authenticate_and_multiplex(tcp_with_dns.boxed(), &identity).unwrap();

    let peer_id = PeerId::from(identity.public());

    let behaviour = behaviour::Behaviour::new(
        identity,
        rendezvous_point_peer_id,
        rendezvous_point_address.clone(),
        cli.namespace.clone(),
    );

    let mut swarm = SwarmBuilder::new(transport, behaviour, peer_id)
        .executor(Box::new(|f| {
            tokio::spawn(f);
        }))
        .build();

    println!("Local peer id: {}", swarm.local_peer_id());

    let _ = swarm.listen_on(format!("/ip4/0.0.0.0/tcp/{}", cli.port).parse().unwrap());

    let _ = swarm.add_external_address(cli.external_addr, AddressScore::Infinite);

    while let Some(event) = swarm.next().await {
        match event {
            SwarmEvent::NewListenAddr(addr) => {
                println!("Listening on {}", addr);
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                cause: Some(error),
                ..
            } if peer_id == rendezvous_point_peer_id => {
                println!("Lost connection to rendezvous point {}", error);
            }
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                println!(
                    "Connection established with: {}: {} ",
                    endpoint.get_remote_address(),
                    peer_id
                );
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

pub mod rendezous {
    use super::*;
    use std::pin::Pin;

    #[derive(PartialEq)]
    enum ConnectionStatus {
        Disconnected,
        Dialling,
        Connected,
    }

    enum RegistrationStatus {
        RegisterOnNextConnection,
        Pending,
        Registered {
            re_register_in: Pin<Box<tokio::time::Sleep>>,
        },
    }

    pub struct Behaviour {
        inner: libp2p::rendezvous::Rendezvous,
        rendezvous_peer_id: PeerId,
        rendezvous_address: Multiaddr,
        namespace: String,
        registration_status: RegistrationStatus,
        connection_status: ConnectionStatus,
        registration_ttl: Option<u64>,
    }

    impl Behaviour {
        pub fn new(
            identity: identity::Keypair,
            rendezvous_peer_id: PeerId,
            rendezvous_address: Multiaddr,
            namespace: String,
            registration_ttl: Option<u64>,
        ) -> Self {
            Self {
                inner: libp2p::rendezvous::Rendezvous::new(
                    identity,
                    libp2p::rendezvous::Config::default(),
                ),
                rendezvous_peer_id,
                rendezvous_address,
                namespace,
                registration_status: RegistrationStatus::RegisterOnNextConnection,
                connection_status: ConnectionStatus::Disconnected,
                registration_ttl,
            }
        }

        fn register(&mut self) {
            self.inner.register(
                Namespace::new(self.namespace.clone()).unwrap(),
                self.rendezvous_peer_id,
                self.registration_ttl,
            );
        }
    }

    impl NetworkBehaviour for Behaviour {
        type ProtocolsHandler =
            <libp2p::rendezvous::Rendezvous as NetworkBehaviour>::ProtocolsHandler;
        type OutEvent = libp2p::rendezvous::Event;

        fn new_handler(&mut self) -> Self::ProtocolsHandler {
            self.inner.new_handler()
        }

        fn addresses_of_peer(&mut self, peer_id: &PeerId) -> Vec<Multiaddr> {
            if peer_id == &self.rendezvous_peer_id {
                return vec![self.rendezvous_address.clone()];
            }

            vec![]
        }

        fn inject_connected(&mut self, peer_id: &PeerId) {
            if peer_id == &self.rendezvous_peer_id {
                self.connection_status = ConnectionStatus::Connected;

                match &self.registration_status {
                    RegistrationStatus::RegisterOnNextConnection => {
                        self.register();
                        self.registration_status = RegistrationStatus::Pending;
                    }
                    RegistrationStatus::Registered { .. } => {}
                    RegistrationStatus::Pending => {}
                }
            }
        }

        fn inject_disconnected(&mut self, peer_id: &PeerId) {
            if peer_id == &self.rendezvous_peer_id {
                self.connection_status = ConnectionStatus::Disconnected;
            }
        }

        fn inject_event(
            &mut self,
            peer_id: PeerId,
            connection: ConnectionId,
            event: <<Self::ProtocolsHandler as IntoProtocolsHandler>::Handler as ProtocolsHandler>::OutEvent,
        ) {
            self.inner.inject_event(peer_id, connection, event)
        }

        fn inject_dial_failure(&mut self, peer_id: &PeerId) {
            if peer_id == &self.rendezvous_peer_id {
                self.connection_status = ConnectionStatus::Disconnected;
            }
        }

        #[allow(clippy::type_complexity)]
        fn poll(&mut self, cx: &mut std::task::Context<'_>, params: &mut impl PollParameters) -> Poll<NetworkBehaviourAction<<<Self::ProtocolsHandler as IntoProtocolsHandler>::Handler as ProtocolsHandler>::InEvent, Self::OutEvent>>{
            match &mut self.registration_status {
                RegistrationStatus::RegisterOnNextConnection => match self.connection_status {
                    ConnectionStatus::Disconnected => {
                        self.connection_status = ConnectionStatus::Dialling;

                        return Poll::Ready(NetworkBehaviourAction::DialPeer {
                            peer_id: self.rendezvous_peer_id,
                            condition: DialPeerCondition::Disconnected,
                        });
                    }
                    ConnectionStatus::Dialling => {}
                    ConnectionStatus::Connected => {
                        self.registration_status = RegistrationStatus::Pending;
                        self.register();
                    }
                },
                RegistrationStatus::Registered { re_register_in } => {
                    if let Poll::Ready(()) = re_register_in.poll_unpin(cx) {
                        match self.connection_status {
                            ConnectionStatus::Connected => {
                                self.registration_status = RegistrationStatus::Pending;
                                self.register();
                            }
                            ConnectionStatus::Disconnected => {
                                self.registration_status =
                                    RegistrationStatus::RegisterOnNextConnection;

                                return Poll::Ready(NetworkBehaviourAction::DialPeer {
                                    peer_id: self.rendezvous_peer_id,
                                    condition: DialPeerCondition::Disconnected,
                                });
                            }
                            ConnectionStatus::Dialling => {}
                        }
                    }
                }
                RegistrationStatus::Pending => {}
            }

            let inner_poll = self.inner.poll(cx, params);

            // reset the timer if we successfully registered
            if let Poll::Ready(NetworkBehaviourAction::GenerateEvent(
                libp2p::rendezvous::Event::Registered { ttl, .. },
            )) = &inner_poll
            {
                let half_of_ttl = Duration::from_secs(*ttl) / 2;

                self.registration_status = RegistrationStatus::Registered {
                    re_register_in: Box::pin(tokio::time::sleep(half_of_ttl)),
                };
            }

            inner_poll
        }
    }
}

pub mod behaviour {
    use super::*;
    use libp2p::ping::Ping;

    /// A `NetworkBehaviour` that registers as a node on a regular interval.
    #[derive(NetworkBehaviour)]
    #[behaviour(out_event = "Event", event_process = false)]
    #[allow(missing_debug_implementations)]
    pub struct Behaviour {
        pub rendezvous: rendezous::Behaviour,

        /// Ping behaviour that ensures that the underlying network connection
        /// is still alive. If the ping fails a connection close event
        /// will be emitted that is picked up as swarm event.
        ping: Ping,
    }

    impl Behaviour {
        pub fn new(
            identity: identity::Keypair,
            rendezvous_peer_id: PeerId,
            rendezvous_address: Multiaddr,
            namespace: String,
        ) -> Self {
            Self {
                rendezvous: rendezous::Behaviour::new(
                    identity,
                    rendezvous_peer_id,
                    rendezvous_address,
                    namespace,
                    None, // use default ttl on rendezvous point
                ),
                ping: Ping::default(),
            }
        }
    }
}
