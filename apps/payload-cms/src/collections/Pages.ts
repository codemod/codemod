import { CollectionConfig } from "payload/types";
import { seoField } from "../fields/shared/seo";
import { publishStatusField } from "../fields/shared/publishStatus";
import {
  heroSectionBlock,
  featuresSectionBlock,
  registrySectionBlock,
  testimonialsSectionBlock,
  fullWidthMediaSectionBlock,
} from "../blocks/sectionBlocks";

const Pages: CollectionConfig = {
  slug: "pages",
  admin: {
    useAsTitle: "title",
    defaultColumns: ["title", "pathname", "updatedAt"],
  },
  access: {
    read: () => true,
  },
  versions: {
    drafts: true,
  },
  fields: [
    {
      name: "title",
      type: "text",
      label: "Title",
      admin: {
        description: "Content group",
      },
    },
    {
      name: "pathname",
      type: "text",
      required: true,
      unique: true,
      label: "Pathname",
      admin: {
        description: "URL path for this page (e.g. /about)",
      },
    },
    {
      name: "internalTitle",
      type: "text",
      label: "Internal title",
      admin: {
        description:
          "This title is only used internally, it won't be displayed on the website.",
      },
    },
    {
      name: "hero",
      type: "blocks",
      label: "Hero Section",
      blocks: [heroSectionBlock],
      admin: {
        description: "Content group",
      },
    },
    {
      name: "sections",
      type: "blocks",
      label: "Sections",
      admin: {
        description: "Content group",
      },
      blocks: [
        featuresSectionBlock,
        registrySectionBlock,
        testimonialsSectionBlock,
        fullWidthMediaSectionBlock,
      ],
    },
    {
      name: "cta",
      type: "relationship",
      relationTo: ["page-ctas", "page-cta-doubles", "page-cta-triples"],
      label: "Page Call to action (Optional)",
      admin: {
        description:
          "Call to action for a page. This is placed at the bottom of the page before the footer.",
      },
    },
    publishStatusField,
    seoField,
  ],
};

export default Pages;
