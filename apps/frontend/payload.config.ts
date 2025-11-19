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
  ],
});
