import { GlobalConfig } from "payload";
import { publishStatusField } from "../fields/shared/publishStatus";
import { heroSectionBlock } from "../blocks/sectionBlocks";
import {
  featuresSectionBlock,
  registrySectionBlock,
  testimonialsSectionBlock,
  fullWidthMediaSectionBlock,
} from "../blocks/sectionBlocks";

export const Home: GlobalConfig = {
  slug: "home",
  admin: {
    group: "Pages",
  },
  access: {
    read: () => true,
  },
  versions: {
    drafts: true,
  },
  fields: [
    // Page fields (from definePage)
    {
      name: "pathname",
      type: "text",
      defaultValue: "/",
      admin: {
        readOnly: true,
        description: "Fixed pathname for the home page",
      },
    },
    {
      name: "internalTitle",
      type: "text",
      defaultValue: "Home",
      admin: {
        description:
          "This title is only used internally in Payload, it won't be displayed on the website.",
      },
    },
    publishStatusField,
    // SEO handled by @payloadcms/plugin-seo (adds 'meta' field automatically)

    // Modular page fields (matching Sanity page.ts structure)
    {
      name: "title",
      type: "text",
      label: "Title",
      admin: {
        description: "Optional page title",
      },
    },
    {
      name: "hero",
      type: "blocks",
      label: "Hero Section",
      blocks: [heroSectionBlock],
      admin: {
        description: "Hero section for the home page",
      },
    },
    {
      name: "sections",
      type: "blocks",
      label: "Sections",
      blocks: [
        featuresSectionBlock,
        registrySectionBlock,
        testimonialsSectionBlock,
        fullWidthMediaSectionBlock,
      ],
      admin: {
        description: "Dynamic sections for the home page",
      },
    },
    {
      name: "cta",
      type: "relationship",
      relationTo: "ctas",
      label: "Page CTA (Optional)",
      admin: {
        description:
          "Call to action for a page. This is placed at the bottom of the page before the footer.",
      },
    },
  ],
};

export default Home;
