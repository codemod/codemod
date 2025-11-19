import { CollectionConfig } from "payload/types";
import { seoField } from "../fields/shared/seo";
import { publishStatusField } from "../fields/shared/publishStatus";
import { imageWithAltField } from "../fields/shared/imageWithAlt";

const BlogCustomerStories: CollectionConfig = {
  slug: "blog-customer-stories",
  admin: {
    useAsTitle: "title",
    defaultColumns: ["title", "pathname", "publishedAt", "updatedAt"],
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
      required: true,
      label: "Article headline",
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
        description: "URL path for this story",
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
      name: "tagline",
      type: "text",
      maxLength: 50,
      label: "Tagline",
      admin: {
        description:
          "Optional. Used on the automation page when with matching tags.",
      },
    },
    {
      ...imageWithAltField,
      name: "featuredImage",
      required: true,
      label: "Featured image",
      admin: {
        description: "Appears in the article's card in the blog",
      },
    },
    {
      name: "publishedAt",
      type: "date",
      required: true,
      label: "Date of first publication",
      admin: {
        description: "Classification group",
      },
    },
    {
      name: "authors",
      type: "relationship",
      relationTo: "blog-authors",
      hasMany: true,
      label: "Author(s)",
      admin: {
        description: "Classification group",
      },
    },
    {
      name: "preamble",
      type: "textarea",
      maxLength: 175,
      label: "Preamble or introduction",
      admin: {
        description:
          "Optional, appears in the article's card in the blog. If none provided, will use the first paragraph of the content.",
      },
    },
    {
      name: "body",
      type: "richText",
      required: true,
      label: "Content",
      admin: {
        description: "Content group",
      },
    },
    {
      name: "sidebar",
      type: "group",
      label: "Sidebar",
      admin: {
        description:
          "These will show in the sidebar of the article. For best results, use no more than 2 items",
      },
      fields: [
        {
          name: "features",
          type: "relationship",
          relationTo: "tech-features",
          hasMany: true,
          label: "Features",
          admin: {
            description:
              "Optional. Select a list of tech features relevant to this story.",
          },
        },
        {
          name: "showArticleCta",
          type: "checkbox",
          label: "Show article CTA",
          admin: {
            description:
              "If checked, the article CTA will be shown in the sidebar of the article.",
          },
        },
        {
          name: "stats",
          type: "array",
          maxRows: 2,
          label: "Stats",
          admin: {
            description:
              "Optional. Add up to 2 stats to show in the sidebar of the article.",
          },
          fields: [
            {
              name: "value",
              type: "text",
              required: true,
              label: "Value",
            },
            {
              name: "label",
              type: "text",
              required: true,
              label: "Label",
            },
          ],
        },
      ],
    },
    {
      name: "pageCta",
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

export default BlogCustomerStories;
