# Screens

The images in `docs/screens/<tag>/` are recorded, not drawn: [VHS](https://github.com/charmbracelet/vhs) types the commands into a real terminal, and turnout - the release's own Linux binary - deploys to a disposable stand in a container that answers as `staging.example.com`. The terminal's colours are those of the line's console surface.

To record them for a release, with Docker:

```bash
docker build -t tn-demo-stand tools/screens/stand
docker network create tn-demo
docker volume create tn-demo-home
docker run -d --rm --name tn-demo-stand --network tn-demo --network-alias staging.example.com tn-demo-stand

# work/bin/turnout: the release's x86_64-unknown-linux-gnu binary
# work/web/dist:    any build - the screens show ~25 MiB of chunks
cp tools/screens/*.tape tools/screens/setup.sh work/
for tape in deploy tour; do
  docker run --rm --network tn-demo -v tn-demo-home:/root -v "$PWD/work:/work" -v "$PWD/out:/out" \
    ghcr.io/charmbracelet/vhs "/work/$tape.tape"
done
```

`deploy.tape` writes `deploy.gif` and `deploy.png`; `tour.tape`, run after it on the same home, writes `status.png`, `doctor-server.png` and `history.png`.
