import { createRequire } from "module";
import path from "path";
import { fileURLToPath } from "url";
import { withPayload } from "@payloadcms/next/withPayload";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

// Conditionally import MonacoEditorPlugin (only available in dev)
// This is needed because it's a devDependency but Next.js config runs during build
let MonacoEditorPlugin = null;
try {
  const require = createRequire(import.meta.url);
  MonacoEditorPlugin = require("monaco-editor-webpack-plugin");
} catch (e) {
  // Monaco editor plugin not available in production builds
  // This is fine - it's only needed for development
  MonacoEditorPlugin = null;
}

/** @type {import('next').NextConfig} */
const config = {
  compiler: {
    styledComponents: true,
  },
  webpack: (config, { isServer, webpack }) => {
    if (!isServer && MonacoEditorPlugin) {
      config.plugins.push(
        new MonacoEditorPlugin({
          languages: ["typescript", "html", "css", "json"],
          filename: "static/[name].worker.js",
          publicPath: "/_next",
        }),
      );
    }

    config.plugins.push(
      new webpack.NormalModuleReplacementPlugin(/node:/, (resource) => {
        resource.request = resource.request.replace(/^node:/, "");
      }),
    );

    // Ensure webpack resolves TypeScript path aliases correctly
    // Next.js reads tsconfig.json automatically, but we ensure aliases match
    const existingAlias = config.resolve.alias || {};
    config.resolve.alias = {
      ...existingAlias,
      // Base path aliases - Next.js should auto-read from tsconfig, but explicit ensures resolution
      "@": path.resolve(__dirname, "."),
      "@payload-config": path.resolve(__dirname, "./payload.config.ts"),
      // Studio paths - these need to match tsconfig.json exactly
      "@studio": path.resolve(
        __dirname,
        "./app/(website)/studio-jscodeshift/src",
      ),
      "@studio/main": path.resolve(
        __dirname,
        "./app/(website)/studio-jscodeshift/main",
      ),
      "@features": path.resolve(
        __dirname,
        "./app/(website)/studio-jscodeshift/features",
      ),
      "@chatbot": path.resolve(
        __dirname,
        "./app/(website)/studio-jscodeshift/features/modGPT",
      ),
      "@gr-run": path.resolve(
        __dirname,
        "./app/(website)/studio-jscodeshift/features/GHRun",
      ),
      "@utils": path.resolve(__dirname, "./utils"),
      "@context": path.resolve(__dirname, "./app/context"),
      "@auth": path.resolve(__dirname, "./app/auth"),
      "@mocks": path.resolve(__dirname, "./mocks"),
    };

    // Add txt loader rule
    config.module.rules.push({
      test: /\.txt$/i,
      use: "raw-loader",
    });

    // Update resolve fallbacks
    config.resolve.fallback = {
      ...config.resolve.fallback,
      fs: false,
      crypto: false,
      buffer: false,
      stream: false,
      child_process: false,
    };

    return config;
  },
  images: {
    remotePatterns: [{ hostname: "cdn.sanity.io" }],
  },
  typescript: {
    // Set this to false if you want production builds to abort if there's type errors
    ignoreBuildErrors: true,
  },
  eslint: {
    /// Set this to false if you want production builds to abort if there's lint errors
    ignoreDuringBuilds: true,
  },
  logging: {
    fetches: {
      fullUrl: true,
    },
  },
  experimental: {
    taint: true,
    optimizePackageImports: [
      "@phosphor-icons/react",
      "lucide-react",
      "@radix-ui/react-select",
      "@radix-ui/react-dialog",
      "@radix-ui/react-tooltip",
    ],
  },
  transpilePackages: [
    "@codemod-com/api-types",
    "@codemod-com/utilities",
    "@codemod.com/codemod-utils",
    "@payloadcms/ui",
  ],
  async headers() {
    return [
      {
        // Exclude Payload admin routes - they handle their own CSP
        source: "/:path((?!admin|api/payload).)*",
        headers: [
          {
            key: "Content-Security-Policy",
            value:
              "default-src 'self'; " +
              "script-src 'self' https://www.google.com/recaptcha/ https://www.gstatic.com/recaptcha/ https://cdn.jsdelivr.net 'unsafe-inline' 'unsafe-eval'; " +
              "frame-src https://www.google.com/recaptcha/ https://www.gstatic.com/recaptcha/; " +
              "style-src 'self' 'unsafe-inline'; " +
              "img-src * data: blob:; " +
              "connect-src *;",
          },
        ],
      },
    ];
  },
  async redirects() {
    return [
      {
        source: "/studio",
        has: [
          {
            type: "query",
            key: "c",
          },
        ],
        destination: "/studio-jscodeshift",
        permanent: true,
      },

      {
        source: "/studio",
        destination: "https://app.codemod.com/studio",
        permanent: false,
      },
      {
        source: "/automations/eslint-to-biome-migrate-rules/",
        destination: "/registry/biome-migrate-rules",
        permanent: false,
      },
      {
        source: "/automations/mocha-to-vitest-migration-recipe/",
        destination: "/registry/mocha-vitest-recipe",
        permanent: false,
      },
      {
        source: "/automations/:slug*",
        destination: "/registry/:slug*",
        permanent: true,
      },
    ];
  },
};

export default withPayload(config);
