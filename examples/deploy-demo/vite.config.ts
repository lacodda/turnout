import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The build stamp is what makes a deploy visible: if the page shows a time
// earlier than the build you just ran, the upload did not land.
const buildStamp = new Date().toISOString().replace("T", " ").slice(0, 19);

export default defineConfig({
  plugins: [react()],
  // Assets are referenced relatively so the app works from any directory a
  // server maps it to, not just the domain root.
  base: "./",
  define: {
    __BUILD_STAMP__: JSON.stringify(buildStamp),
  },
});
