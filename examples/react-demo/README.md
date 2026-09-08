# react-demo

A small Vite + React app that exists to show the gateway at work: the
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
turnout app add react-demo --path examples/react-demo --port 7102 --env-var VITE_API_URL --server httpbin
turnout use react-demo httpbin
```

Every day:

```bash
turnout gateway start
turnout dev react-demo          # the port comes from turnout; the address is by name
turnout open react-demo         # http://react-demo.localhost
```

What to look at on the page:

- **Served at** - `http://react-demo.localhost`, the app's own address through the
  gateway's front door, whatever port the dev server took.
- **Gateway** - `VITE_API_URL`, handed to the dev server by `turnout dev` (and
  kept in `.env.development.local` for a server started by hand).
- **Ping the stand** - the request goes to `localhost:7102` and comes
  back from httpbin, which reports the URL and headers it saw.
- **Set a cookie / Read cookies** - the stand sets a cookie; the browser never
  sees it (`document.cookie` stays empty), the gateway's jar keeps it and
  sends it back on the next request. Switch stands and the jar switches too.

Nothing in this directory names a port or a stand: that is the point.
