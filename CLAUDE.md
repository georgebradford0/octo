# claudulhu — project notes for Claude

## Docker image

The correct image name is **`ghcr.io/georgebradford0/claudulhu-server`**.

Pull:
```sh
docker pull ghcr.io/georgebradford0/claudulhu-server:latest
```

Build and push (replace `X.Y.Z` with the new version). Always use `buildx` with `--platform` so both `linux/amd64` and `linux/arm64` are included in the manifest:
```sh
docker buildx build \
  --builder multiplatform \
  --platform linux/amd64,linux/arm64 \
  --push \
  -t ghcr.io/georgebradford0/claudulhu-server:X.Y.Z \
  -t ghcr.io/georgebradford0/claudulhu-server:latest \
  .
```

**Never** use `claudulhu:latest` or any name that omits `-server`.
