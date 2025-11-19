import { CollectionConfig } from "payload";
import { publishStatusField } from "./fields/shared/publishStatus";
import { heroSectionBlock } from "./blocks/sectionBlocks";
import {
  featuresSectionBlock,
  registrySectionBlock,
  testimonialsSectionBlock,
  fullWidthMediaSectionBlock,
} from "./blocks/sectionBlocks";
import { formatSlug } from "./utils/formatSlug";

export const Pages: CollectionConfig = {
  slug: "pages",
  admin: {
    group: "Pages",
    useAsTitle: "internalTitle",
    defaultColumns: ["internalTitle", "pageType", "pathname", "updatedAt"],
  },
  access: {
    read: () => true,
  },
  versions: {
    drafts: true,
  },
  fields: [
    // Common fields
    {
      name: "pathname",
      type: "text",
      required: true,
      unique: true,
      label: "Pathname",
      admin: {
        description:
          "URL path for this page. Auto-generated from internal title if left empty. You can override with any path.",
      },
      hooks: {
        beforeChange: [
          ({ value, data, operation }) => {
            // If pathname is empty and internalTitle exists, auto-generate
            if (
              !value &&
              data?.internalTitle &&
              typeof data.internalTitle === "string"
            ) {
              const slug = formatSlug(data.internalTitle);
              if (slug) {
                return `/${slug}`;
              }
            }
            // Allow manual override - return whatever user entered
            return value;
          },
        ],
      },
    },
    {
      name: "internalTitle",
      type: "text",
      required: true,
      label: "Internal title",
      admin: {
        description:
          "This title is only used internally in Payload, it won't be displayed on the website.",
      },
    },
    publishStatusField,
    // SEO handled by @payloadcms/plugin-seo (adds 'meta' field automatically)

    // Page type selector
    {
      name: "pageType",
      type: "select",
      required: true,
      label: "Page Type",
      options: [
        { label: "Modular Page", value: "modular" },
        { label: "Text Page", value: "text" },
      ],
      admin: {
        description: "Select the type of page you want to create",
      },
    },

    // Title field (optional for modular, required for text)
    {
      name: "title",
      type: "text",
      label: "Title",
      admin: {
        description:
          "Page title. Required for text pages, optional for modular pages.",
      },
      validate: (value, { data }) => {
        if (data?.pageType === "text" && !value) {
          return "Title is required for text pages";
        }
        return true;
      },
    },

    // Common to modular pages: Hero section
    {
      name: "hero",
      type: "blocks",
      label: "Hero Section",
      blocks: [heroSectionBlock],
      admin: {
        condition: (data) => data?.pageType === "modular",
        description: "Hero section for modular pages",
      },
    },

    // Modular pages: Dynamic sections
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
        condition: (data) => data?.pageType === "modular",
        description: "Dynamic sections for modular pages",
      },
    },

    // Modular pages: Page CTA
    {
      name: "pageCta",
      type: "relationship",
      relationTo: "ctas",
      label: "Page CTA (Optional)",
      admin: {
        condition: (data) => data?.pageType === "modular",
        description:
          "Call to action for a page. This is placed at the bottom of the page before the footer.",
      },
    },

    // Text page-specific fields
    {
      name: "lastUpdatedText",
      type: "text",
      label: "Last updated text",
      admin: {
        condition: (data) => data?.pageType === "text",
        description: 'Optional. ex.: "Last updated at"',
      },
    },
    {
      name: "tocTitle",
      type: "text",
      required: true,
      label: "Table of contents title",
      admin: {
        condition: (data) => data?.pageType === "text",
      },
    },
    {
      name: "body",
      type: "richText",
      required: true,
      label: "Content",
      admin: {
        condition: (data) => data?.pageType === "text",
        description: "Main page content with rich text formatting",
      },
    },
  ],
};

export default Pages;
