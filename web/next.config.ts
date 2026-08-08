import path from "node:path";
import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // Pin the project root: a sibling package-lock.json elsewhere under
  // ~/github (unrelated projects) otherwise confuses Turbopack's
  // nearest-lockfile root inference.
  turbopack: {
    root: path.resolve(__dirname),
  },
};

export default nextConfig;
