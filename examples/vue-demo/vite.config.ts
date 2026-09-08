import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// The dev server takes its port from turnout: `turnout dev` passes it as
// `--port {port}` on the command line and as PORT in the environment. Either
// road lands here; a port nobody handed over falls back to Vite's own.
// `strictPort` keeps the server honest - a taken port is an error, not a
// silent move to the next one, which the front door would never find.
const port = Number(process.env.PORT) || undefined;

export default defineConfig({
  plugins: [vue()],
  server: { port, strictPort: port !== undefined },
});
