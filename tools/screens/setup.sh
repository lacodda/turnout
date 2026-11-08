# Sourced, hidden, at the start of every tape: a catalog that reaches the
# demo stand, nothing of the machine that records it.
export LANG=C.UTF-8 LC_ALL=C.UTF-8 PATH=/work/bin:$PATH TURNOUT_KEYRING=insecure-file TURNOUT_UPDATE_CHECK=0 TURNOUT_TOASTS=/tmp/toasts.jsonl
cd /work/web
if [ ! -d "$HOME/.local/share/lacodda/turnout" ]; then
  turnout setup -y >/dev/null
  turnout credential add deploy --user deploy --auth password >/dev/null
  printf sesame | turnout pass set deploy >/dev/null
  turnout server add staging --url http://staging.example.com --credential deploy >/dev/null
  turnout path add web-root --dir /srv/web --restart web-reload >/dev/null
  turnout app add web --path /work/web --dist dist --server staging >/dev/null 2>&1
  turnout target add web-staging --app web --server staging --credential deploy --path web-root --health /health >/dev/null
fi
clear
