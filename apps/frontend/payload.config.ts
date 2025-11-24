import { postgresAdapter } from "@payloadcms/db-postgres";
import { lexicalEditor } from "@payloadcms/richtext-lexical";
import { seoPlugin } from "@payloadcms/plugin-seo";
import path from "path";
import { buildConfig } from "payload";
import { fileURLToPath } from "url";
import sharp from "sharp";

import { Users } from "./payload/Users";
import Media from "./payload/Media";
import BlogAuthors from "./payload/BlogAuthors";
import BlogTags from "./payload/BlogTags";
import BlogPosts from "./payload/BlogPosts";
import Pages from "./payload/Pages";
import Job from "./payload/Job";
import CTAs from "./payload/CTAs";
import { richTextBlocks } from "./payload/blocks/richTextBlocks";

// Globals
import Home from "./payload/globals/Home";
import About from "./payload/globals/About";
import Pricing from "./payload/globals/Pricing";
import Contact from "./payload/globals/Contact";
import Careers from "./payload/globals/Careers";
import Settings from "./payload/globals/Settings";
import Navigation from "./payload/globals/Navigation";
import Footer from "./payload/globals/Footer";
import GlobalLabels from "./payload/globals/GlobalLabels";
import FrameworkIcons from "./payload/globals/FrameworkIcons";
import RegistryIndex from "./payload/globals/RegistryIndex";
import BlogIndex from "./payload/globals/BlogIndex";
import NotFound from "./payload/globals/NotFound";

const filename = fileURLToPath(import.meta.url);
const dirname = path.dirname(filename);

// Conditionally load storage adapter based on environment variables
const getStorageAdapter = () => {
  // Vercel Blob (recommended for Vercel deployments)
  if (process.env.BLOB_READ_WRITE_TOKEN) {
    try {
      // Dynamic import to avoid requiring the package in local dev
      const { vercelBlobStorage } = require("@payloadcms/storage-vercel-blob");
      return vercelBlobStorage({
        collections: {
          media: {
            token: process.env.BLOB_READ_WRITE_TOKEN,
          },
        },
      });
    } catch (error) {
      console.warn(
        "⚠️  @payloadcms/storage-vercel-blob not installed. Install with: pnpm add @payloadcms/storage-vercel-blob",
      );
      return null;
    }
  }

  // AWS S3
  if (process.env.S3_BUCKET && process.env.S3_ACCESS_KEY_ID) {
    try {
      const { s3Storage } = require("@payloadcms/storage-s3");
      return s3Storage({
        collections: {
          media: {
            bucket: process.env.S3_BUCKET,
            prefix: "media",
          },
        },
        config: {
          credentials: {
            accessKeyId: process.env.S3_ACCESS_KEY_ID,
            secretAccessKey: process.env.S3_SECRET_ACCESS_KEY,
          },
          region: process.env.S3_REGION || "us-east-1",
          endpoint: process.env.S3_ENDPOINT,
        },
      });
    } catch (error) {
      console.warn(
        "⚠️  @payloadcms/storage-s3 not installed. Install with: pnpm add @payloadcms/storage-s3",
      );
      return null;
    }
  }

  // Cloudflare R2
  if (process.env.R2_BUCKET && process.env.R2_ACCOUNT_ID) {
    try {
      const { r2Storage } = require("@payloadcms/storage-r2");
      return r2Storage({
        collections: {
          media: {
            bucket: process.env.R2_BUCKET,
            prefix: "media",
          },
        },
        config: {
          accountId: process.env.R2_ACCOUNT_ID,
          accessKeyId: process.env.R2_ACCESS_KEY_ID,
          secretAccessKey: process.env.R2_SECRET_ACCESS_KEY,
        },
      });
    } catch (error) {
      console.warn(
        "⚠️  @payloadcms/storage-r2 not installed. Install with: pnpm add @payloadcms/storage-r2",
      );
      return null;
    }
  }

  // No storage adapter configured - will use local file system (fine for local dev)
  return null;
};

const storageAdapter = getStorageAdapter();

export default buildConfig({
  admin: {
    user: Users.slug,
    importMap: {
      baseDir: path.resolve(dirname),
    },
  },
  collections: [
    Users,
    Media,
    BlogAuthors,
    BlogTags,
    BlogPosts,
    Pages,
    Job,
    CTAs,
  ],
  globals: [
    Home,
    About,
    Pricing,
    Contact,
    Careers,
    Settings,
    Navigation,
    Footer,
    GlobalLabels,
    FrameworkIcons,
    RegistryIndex,
    BlogIndex,
    NotFound,
  ],
  editor: lexicalEditor(),
  secret: process.env.PAYLOAD_SECRET || "",
  typescript: {
    outputFile: path.resolve(dirname, "payload-types.ts"),
  },
  db: postgresAdapter({
    pool: {
      // Support both DATABASE_URL (Neon/Vercel standard) and DATABASE_URI (current)
      connectionString:
        process.env.DATABASE_URL || process.env.DATABASE_URI || "",
    },
  }),
  sharp,
  plugins: [
    seoPlugin({
      collections: ["blog-posts", "pages", "jobs"],
      globals: [
        "home",
        "about",
        "pricing",
        "contact",
        "careers",
        "blog-index",
        "registry-index",
      ],
      uploadsCollection: "media",
    }),
    // Add storage adapter if configured
    ...(storageAdapter ? [storageAdapter] : []),
  ],
});
