import { postgresAdapter } from "@payloadcms/db-postgres";
import { lexicalEditor } from "@payloadcms/richtext-lexical";
import { seoPlugin } from "@payloadcms/plugin-seo";
import path from "path";
import { buildConfig } from "payload";
import { fileURLToPath } from "url";
import sharp from "sharp";

// Storage adapters - uncomment the one you want to use
// import { s3Storage } from "@payloadcms/storage-s3";
// import { r2Storage } from "@payloadcms/storage-r2";
// import { vercelBlobStorage } from "@payloadcms/storage-vercel-blob";

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
      connectionString: process.env.DATABASE_URI || "",
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

    // Uncomment and configure ONE storage adapter based on your choice:

    // Option 1: AWS S3
    // s3Storage({
    //   collections: {
    //     media: {
    //       bucket: process.env.S3_BUCKET!,
    //       prefix: "media",
    //     },
    //   },
    //   config: {
    //     credentials: {
    //       accessKeyId: process.env.S3_ACCESS_KEY_ID!,
    //       secretAccessKey: process.env.S3_SECRET_ACCESS_KEY!,
    //     },
    //     region: process.env.S3_REGION || "us-east-1",
    //     endpoint: process.env.S3_ENDPOINT, // Optional, for S3-compatible services
    //   },
    // }),

    // Option 2: Cloudflare R2
    // r2Storage({
    //   collections: {
    //     media: {
    //       bucket: process.env.R2_BUCKET!,
    //       prefix: "media",
    //     },
    //   },
    //   config: {
    //     accountId: process.env.R2_ACCOUNT_ID!,
    //     accessKeyId: process.env.R2_ACCESS_KEY_ID!,
    //     secretAccessKey: process.env.R2_SECRET_ACCESS_KEY!,
    //   },
    // }),

    // Option 3: Vercel Blob (Simplest)
    // vercelBlobStorage({
    //   collections: {
    //     media: {
    //       token: process.env.BLOB_READ_WRITE_TOKEN!,
    //     },
    //   },
    // }),
  ],
});
