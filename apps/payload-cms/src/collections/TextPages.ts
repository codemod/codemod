import { CollectionConfig } from "payload/types";
import { seoField } from "../fields/shared/seo";
import { publishStatusField } from "../fields/shared/publishStatus";
import {
  codeSnippetBlock,
  imageBlock,
  youtubeVideoBlock,
  muxVideoWithCaptionBlock,
  quoteBlock,
  tableBlock,
  twitterEmbedBlock,
  linkedImageBlock,
} from "../blocks/richTextBlocks";

const TextPages: CollectionConfig = {
  slug: "text-pages",
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
      required: true,
      label: "Page title",
      admin: {
        description: "Contains page title. Content group",
      },
    },
    {
      name: "pathname",
      type: "text",
      required: true,
      unique: true,
      label: "Pathname",
      admin: {
        description: "URL path for this page",
      },
    },
    {
      name: "internalTitle",
      type: "text",
      label: "Internal title",
    },
    {
      name: "lastUpdatedText",
      type: "text",
      label: "Last updated text",
      admin: {
        description: "Optional. ex.: `Last updated at`",
      },
    },
    {
      name: "tocTitle",
      type: "text",
      required: true,
      label: "Table of contents title",
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
    publishStatusField,
    seoField,
  ],
};

export default TextPages;
