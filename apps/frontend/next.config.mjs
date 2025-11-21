import MonacoEditorPlugin from "monaco-editor-webpack-plugin";
import { withPayload } from "@payloadcms/next/withPayload";

/** @type {import('next').NextConfig} */
const config = {
  compiler: {
    styledComponents: true,
  },
  webpack: (config, { isServer, webpack }) => {
    if (!isServer) {
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

    return {
      ...config,
      module: {
        ...config.module,
        rules: [
          ...config.module.rules,
          {
            test: /\.txt$/i,
            use: "raw-loader",
          },
        ],
      },
      resolve: {
        ...config.resolve,
        fallback: {
          ...config.resolve.fallback,
          fs: false,
          crypto: false,
          buffer: false,
          stream: false,
          child_process: false,
        },
      },
    };
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
