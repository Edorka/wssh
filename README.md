# wssh
WebSocket to SSH
 

# Trying it out

```
cargo run -- -p <WS PORT> -a <SSH HOST> -d
```

# Configuration

- `-t`, `--token-rotation-period <SECONDS>`: period after which the session
  token is rotated. The new token is delivered in `Pong` responses and only
  becomes effective once the client acknowledges it with an `Acknowledge`
  message. Default: `300`. Use `0` to disable rotation.
