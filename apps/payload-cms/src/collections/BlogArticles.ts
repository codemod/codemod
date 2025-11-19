import { CollectionConfig } from "payload/types";
import { seoField } from "../fields/shared/seo";
import { publishStatusField } from "../fields/shared/publishStatus";
import { imageWithAltField } from "../fields/shared/imageWithAlt";
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
} from "../blocks/richTextBlocks";

const BlogArticles: CollectionConfig = {
  slug: "blog-articles",
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
        description: "URL path for this article (e.g. /blog/my-article)",
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
      ...imageWithAltField,
      name: "featuredImage",
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
      name: "tags",
      type: "relationship",
      relationTo: "blog-tags",
      hasMany: true,
      maxRows: 1,
      label: "Article tag",
      admin: {
        description:
          "Highly recommended to tag the article for search and filtering purposes. Classification group",
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
        description: "These will show in the sidebar of the article.",
      },
      fields: [
        {
          name: "showToc",
          type: "checkbox",
          defaultValue: true,
          label: "Show Table of Contents?",
          admin: {
            description:
              "If checked, a table of contents will be generated from the headings in the article.",
          },
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

export default BlogArticles;
