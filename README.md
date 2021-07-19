# Registration and Discovery Examples for Rendezvous Server

1. Start [rendezvous server](https://github.com/comit-network/rendezvous-server)
2. Run registration:
   1. For one-time registration run:
   ```bash
      cargo run --package rendezvous-client --bin registration -- \ 
      --rendezvous-addr RENDEZVOUS_SERVER_MULTIADDR \
      --rendezvous-peer-id RENDEZVOUS_SERVER_PEER_ID \
      --external-addr /ip4/127.0.0.1/tcp/9999 \
      --secret-file /path/to/secret/file \
      --port 9999 \
      --namespace SOME_NAMESPACE
   ```

   2. For repetitive registration run:
   ```bash
      cargo run --package rendezvous-client --bin repetitive -- \
      --rendezvous-addr RENDEZVOUS_SERVER_MULTIADDR \
      --rendezvous-peer-id RENDEZVOUS_SERVER_PEER_ID \
      --external-addr /ip4/127.0.0.1/tcp/9999 \
      --secret-file /path/to/secret/file \
      --port 9999 \
      --namespace SOME_NAMESPACE
   ```

Initially you can use `--generate-secret` to generate a secret file.

3. Run discovery:

```bash
cargo run --package rendezvous-client --bin discovery -- 
--rendezvous-addr RENDEZVOUS_SERVER_MULTIADDR
--rendezvous-peer-id RENDEZVOUS_SERVER_PEER_ID
--namespace SOME_NAMESPACE
```
