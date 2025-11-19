import { buildConfig } from "payload/config";
import { postgresAdapter } from "@payloadcms/db-postgres";
import { lexicalEditor } from "@payloadcms/richtext-lexical";
import path from "path";
import { fileURLToPath } from "url";
import {
  codeSnippetBlock,
  imageBlock,
  youtubeVideoBlock,
  muxVideoWithCaptionBlock,
  quoteBlock,
  tableBlock,
  collapsibleBlock,
  twitterEmbedBlock,
  linkedImageBlock,
} from "./blocks/richTextBlocks";

const filename = fileURLToPath(import.meta.url);
const dirname = path.dirname(filename);

// Collections
import BlogArticles from "./collections/BlogArticles";
import BlogCustomerStories from "./collections/BlogCustomerStories";
import BlogAuthors from "./collections/BlogAuthors";
import BlogTags from "./collections/BlogTags";
import BlogIndex from "./collections/BlogIndex";
import Pages from "./collections/Pages";
import TextPages from "./collections/TextPages";
import Navigation from "./collections/Navigation";
import Footer from "./collections/Footer";
import Settings from "./collections/Settings";
import GlobalLabels from "./collections/GlobalLabels";
import Jobs from "./collections/Jobs";
import Careers from "./collections/Careers";
import Pricing from "./collections/Pricing";
import Contact from "./collections/Contact";
import About from "./collections/About";
import NotFound from "./collections/NotFound";
import PageCta from "./collections/PageCta";
import PageCtaDouble from "./collections/PageCtaDouble";
import PageCtaTriple from "./collections/PageCtaTriple";
import ArticleCta from "./collections/ArticleCta";
import TechFeature from "./collections/TechFeature";
import RegistryIndex from "./collections/RegistryIndex";
import FilterIconDictionary from "./collections/FilterIconDictionary";
import Media from "./collections/Media";
import Users from "./collections/Users";

export default buildConfig({
  admin: {
    user: "users",
    baseURL: process.env.PAYLOAD_PUBLIC_SERVER_URL || "http://localhost:3001",
    importMap: {
      baseDir: path.resolve(dirname),
    },
  },
  serverURL: process.env.PAYLOAD_PUBLIC_SERVER_URL || "http://localhost:3001",
  routes: {
    admin: "/admin",
    api: "/api",
  },
  collections: [
    // Authentication
    Users,
    // Blog collections
    BlogArticles,
    BlogCustomerStories,
    BlogAuthors,
    BlogTags,
    BlogIndex,
    // Page collections
    Pages,
    TextPages,
    // Global/Singleton collections
    Navigation,
    Footer,
    Settings,
    GlobalLabels,
    // Supporting collections
    Jobs,
    Careers,
    Pricing,
    Contact,
    About,
    NotFound,
    PageCta,
    PageCtaDouble,
    PageCtaTriple,
    ArticleCta,
    TechFeature,
    RegistryIndex,
    FilterIconDictionary,
    // Media
    Media,
  ],
  editor: lexicalEditor({
    features: ({ defaultFeatures }) => [
      ...defaultFeatures,
      // Custom blocks will be configured at the field level
    ],
  }),
  secret: process.env.PAYLOAD_SECRET || "",
  typescript: {
    outputFile: path.resolve(dirname, "payload-types.ts"),
  },
  db: postgresAdapter({
    pool: {
      connectionString: process.env.DATABASE_URI || "",
    },
  }),
  plugins: [],
});
