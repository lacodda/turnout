# vue-demo

A small Vite + Vue 3 app that exists to show the gateway at work: the
page says where it is served from, which gateway it was handed, and what a
request through that gateway looks like on the other side.

It talks to whichever stand the app is bound to. Any HTTP server will do;
[httpbin.org](https://httpbin.org) is used below because it answers with the
request it received, which makes the routing visible.

```bash
pnpm install
```

Wire it into turnout - once:

```bash
turnout server add httpbin --url https://httpbin.org
turnout app add vue-demo --path examples/vue-demo --port 7101 --env-var VITE_API_URL --server httpbin
turnout use vue-demo httpbin
```

Every day:

```bash
turnout gateway start
turnout dev vue-demo          # the port comes from turnout; the address is by name
turnout open vue-demo         # http://vue-demo.localhost
```

What to look at on the page:

- **Served at** - `http://vue-demo.localhost`, the app's own address through the
  gateway's front door, whatever port the dev server took.
- **Gateway** - `VITE_API_URL`, handed to the dev server by `turnout dev` (and
  kept in `.env.development.local` for a server started by hand).
- **Ping the stand** - the request goes to `localhost:7101` and comes
  back from httpbin, which reports the URL and headers it saw.
- **Set a cookie / Read cookies** - the stand sets a cookie; the browser never
  sees it (`document.cookie` stays empty), the gateway's jar keeps it and
  sends it back on the next request. Switch stands and the jar switches too.

Nothing in this directory names a port or a stand: that is the point.
