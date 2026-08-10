# deploy-demo

A deliberately tiny Vite + React app used to exercise `turnout deploy` against a
real server. The page shows nothing but the time its bundle was built, which is
what makes a deploy verifiable: reload after deploying and the stamp moves, or
the upload did not land.

```bash
pnpm install
pnpm build          # writes dist/
```

Wire it into turnout and send it somewhere:

```bash
turnout app add deploy-demo -p examples/deploy-demo -d dist
turnout deploy-setup deploy-demo -s mystand
turnout deploy deploy-demo
```

The app is not published and has no runtime dependencies beyond React; it exists
so deploy changes can be tested end to end instead of by eye.
