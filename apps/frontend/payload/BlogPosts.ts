import { CollectionConfig } from "payload";
import { imageWithAltField } from "./fields/shared/imageWithAlt";
import { publishStatusField } from "./fields/shared/publishStatus";
import { formatSlug } from "./utils/formatSlug";
import { richTextBlocks } from "./blocks/richTextBlocks";
import {
  BlocksFeature,
  lexicalEditor,
  CodeBlock,
  EXPERIMENTAL_TableFeature,
} from "@payloadcms/richtext-lexical";

export const BlogPosts: CollectionConfig = {
  slug: "blog-posts",
  admin: {
    group: "Blog",
    useAsTitle: "title",
    defaultColumns: ["title", "postType", "publishedAt", "updatedAt"],
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
          "URL path for this blog post. Auto-generated from title if left empty (defaults to /blog/...). You can override with any path.",
      },
      hooks: {
        beforeChange: [
          ({ value, data, operation }) => {
            // If pathname is empty and title exists, auto-generate
            if (!value && data?.title && typeof data.title === "string") {
              const slug = formatSlug(data.title);
              if (slug) {
                return `/blog/${slug}`;
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
      label: "Internal title",
      admin: {
        description:
          "This title is only used internally in Payload, it won't be displayed on the website.",
      },
    },
    publishStatusField,
    // SEO handled by @payloadcms/plugin-seo (adds 'meta' field automatically)

    // Post type selector
    {
      name: "postType",
      type: "select",
      required: true,
      label: "Post Type",
      options: [
        { label: "Article", value: "article" },
        { label: "Customer Story", value: "customer-story" },
      ],
      admin: {
        description: "Select the type of blog post you want to create",
      },
    },

    // Common blog post fields
    {
      name: "title",
      type: "text",
      required: true,
      label: "Article headline",
    },
    {
      ...imageWithAltField,
      name: "featuredImage",
      label: "Featured image",
      admin: {
        description:
          "Appears in the article's card in the blog. Required for customer stories, optional for articles.",
      },
      validate: (value, { data }) => {
        if (data?.postType === "customer-story" && !value) {
          return "Featured image is required for customer stories";
        }
        return true;
      },
    },
    {
      name: "publishedAt",
      type: "date",
      required: true,
      label: "Date of first publication",
      admin: {
        date: {
          pickerAppearance: "dayAndTime",
        },
      },
    },
    {
      name: "authors",
      type: "relationship",
      relationTo: "blog-authors",
      hasMany: true,
      label: "Author(s)",
    },
    {
      name: "preamble",
      type: "textarea",
      maxLength: 175,
      label: "Preamble or introduction",
      admin: {
        description:
          "Optional, appears in the article's card in the blog. If none provided, will use the first paragraph of the content.",
        rows: 2,
      },
    },
    {
      name: "body",
      type: "richText",
      required: true,
      label: "Content",
      admin: {
        description: "Main article content with rich text formatting",
      },
      editor: lexicalEditor({
        features: ({ defaultFeatures }) => [
          ...defaultFeatures,
          EXPERIMENTAL_TableFeature(), // Built-in experimental table feature
          BlocksFeature({ blocks: [CodeBlock(), ...richTextBlocks] }),
        ],
      }),
    },

    // Article-specific fields
    {
      name: "tags",
      type: "relationship",
      relationTo: "blog-tags",
      hasMany: true,
      maxRows: 1,
      label: "Article tag",
      admin: {
        condition: (data) => data?.postType === "article",
        description:
          "Highly recommended to tag the article for search and filtering purposes.",
      },
    },

    // Customer Story-specific fields
    {
      name: "tagline",
      type: "text",
      maxLength: 50,
      label: "Tagline",
      admin: {
        condition: (data) => data?.postType === "customer-story",
        description:
          "Optional. Used on the automation page when with matching tags.",
      },
    },

    // Sidebar group
    {
      name: "sidebar",
      type: "group",
      label: "Sidebar",
      admin: {
        description: "Sidebar content for the article",
      },
      fields: [
        // Article sidebar: Table of Contents
        {
          name: "showToc",
          type: "checkbox",
          defaultValue: true,
          label: "Show Table of Contents?",
          admin: {
            condition: (data) => data?.postType === "article",
            description:
              "If checked, a table of contents will be generated from the headings in the article.",
          },
        },

        // Customer Story sidebar: Tech Features (embedded array)
        {
          name: "features",
          type: "array",
          label: "Tech Features",
          admin: {
            condition: (data) => data?.postType === "customer-story",
            description:
              "Optional. List of tech features relevant to this story.",
          },
          fields: [
            {
              name: "title",
              type: "text",
              required: true,
              label: "Title",
              admin: {
                description:
                  'Name of the tech feature (e.g., "React", "TypeScript")',
              },
            },
            {
              name: "logo",
              type: "text",
              required: true,
              label: "Logo/Icon Name",
              admin: {
                description:
                  'Icon identifier for TechLogo component (e.g., "react", "typescript")',
              },
            },
            {
              name: "url",
              type: "text",
              required: true,
              label: "URL",
              admin: {
                description:
                  "Link to the tech feature website or documentation",
              },
            },
          ],
        },

        // Customer Story sidebar: Article CTA
        {
          name: "showArticleCta",
          type: "checkbox",
          label: "Show article CTA",
          admin: {
            condition: (data) => data?.postType === "customer-story",
            description:
              "If checked, the article CTA will be shown in the sidebar of the article.",
          },
        },

        // Customer Story sidebar: Stats
        {
          name: "stats",
          type: "array",
          maxRows: 2,
          label: "Stats",
          admin: {
            condition: (data) => data?.postType === "customer-story",
            description:
              "Optional. Add up to 2 stats to show in the sidebar of the article.",
          },
          fields: [
            {
              name: "from",
              type: "text",
              required: true,
              label: "Title",
            },
            {
              name: "useFromTo",
              type: "checkbox",
              label: "Use From → To format",
            },
            {
              name: "to",
              type: "text",
              label: "To",
              admin: {
                condition: (_, siblingData) => siblingData?.useFromTo === true,
                description: "Required when using From → To format",
              },
            },
            {
              name: "subtitle",
              type: "textarea",
              label: "Subtitle",
              admin: {
                rows: 3,
              },
            },
          ],
        },
      ],
    },

    // Page CTA (common to both)
    {
      name: "pageCta",
      type: "relationship",
      relationTo: "ctas",
      label: "Page Call to action (Optional)",
      admin: {
        description:
          "Call to action for a page. This is placed at the bottom of the page before the footer.",
      },
    },
  ],
};

export default BlogPosts;
