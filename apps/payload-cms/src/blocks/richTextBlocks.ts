import { Block } from "payload/types";

// Code Snippet Block
export const codeSnippetBlock: Block = {
  slug: "codeSnippet",
  labels: {
    singular: "Code Snippet",
    plural: "Code Snippets",
  },
  fields: [
    {
      name: "code",
      type: "code",
      required: true,
      label: "Code",
      admin: {
        language: "typescript",
      },
    },
    {
      name: "language",
      type: "select",
      label: "Language",
      options: [
        { label: "TypeScript", value: "typescript" },
        { label: "JavaScript", value: "javascript" },
        { label: "Python", value: "python" },
        { label: "Bash", value: "bash" },
        { label: "JSON", value: "json" },
        { label: "YAML", value: "yaml" },
        { label: "Markdown", value: "markdown" },
        { label: "HTML", value: "html" },
        { label: "CSS", value: "css" },
        { label: "SQL", value: "sql" },
        { label: "Rust", value: "rust" },
        { label: "Go", value: "go" },
      ],
      defaultValue: "typescript",
    },
  ],
};

// Image Block
export const imageBlock: Block = {
  slug: "imageBlock",
  labels: {
    singular: "Image",
    plural: "Images",
  },
  fields: [
    {
      name: "image",
      type: "upload",
      relationTo: "media",
      required: true,
      label: "Image",
    },
    {
      name: "alt",
      type: "text",
      required: true,
      maxLength: 150,
      label: "Alt Text",
    },
    {
      name: "caption",
      type: "text",
      label: "Caption",
    },
  ],
};

// YouTube Video Block
export const youtubeVideoBlock: Block = {
  slug: "youtubeVideo",
  labels: {
    singular: "YouTube Video",
    plural: "YouTube Videos",
  },
  fields: [
    {
      name: "youtubeUrl",
      type: "text",
      required: true,
      label: "Youtube URL",
      admin: {
        description: "Link to youtube embedded video.",
      },
    },
    {
      name: "caption",
      type: "text",
      label: "Caption",
    },
  ],
};

// Mux Video Block
export const muxVideoBlock: Block = {
  slug: "muxVideo",
  labels: {
    singular: "Mux Video",
    plural: "Mux Videos",
  },
  fields: [
    {
      name: "hasControls",
      type: "checkbox",
      label: "Show video controls",
    },
    {
      name: "autoPlay",
      type: "checkbox",
      label: "Auto Play",
      admin: {
        description:
          "If checked, the video will start playing as soon as it's loaded.",
      },
    },
    {
      name: "loop",
      type: "checkbox",
      label: "Loop",
      admin: {
        description:
          "If checked, the video will start over again when it reaches the end.",
      },
    },
    {
      name: "video",
      type: "text",
      label: "Light Mode Video (Mux Playback ID)",
    },
    {
      name: "darkVideo",
      type: "text",
      label: "Dark Mode Video (Mux Playback ID)",
    },
  ],
};

// Mux Video with Caption Block
export const muxVideoWithCaptionBlock: Block = {
  slug: "muxVideoWithCaption",
  labels: {
    singular: "Mux Video with Caption",
    plural: "Mux Videos with Caption",
  },
  fields: [
    ...muxVideoBlock.fields,
    {
      name: "caption",
      type: "text",
      label: "Caption",
    },
  ],
};

// Quote Block
export const quoteBlock: Block = {
  slug: "quoteBlock",
  labels: {
    singular: "Quote",
    plural: "Quotes",
  },
  fields: [
    {
      name: "quote",
      type: "textarea",
      required: true,
      label: "Quote",
    },
    {
      name: "author",
      type: "text",
      label: "Author",
    },
  ],
};

// Table Block
export const tableBlock: Block = {
  slug: "ptTable",
  labels: {
    singular: "Table",
    plural: "Tables",
  },
  fields: [
    {
      name: "table",
      type: "array",
      label: "Table",
      fields: [
        {
          name: "cells",
          type: "array",
          label: "Row",
          fields: [
            {
              name: "cell",
              type: "text",
              label: "Cell",
            },
          ],
        },
      ],
    },
  ],
};

// Collapsible Block
export const collapsibleBlock: Block = {
  slug: "collapsible",
  labels: {
    singular: "Collapsible",
    plural: "Collapsibles",
  },
  fields: [
    {
      name: "title",
      type: "text",
      required: true,
      label: "Title",
    },
    {
      name: "content",
      type: "richText",
      required: true,
      label: "Content",
    },
  ],
};

// Twitter Embed Block
export const twitterEmbedBlock: Block = {
  slug: "twitterEmbed",
  labels: {
    singular: "Twitter Embed",
    plural: "Twitter Embeds",
  },
  fields: [
    {
      name: "tweetId",
      type: "text",
      required: true,
      label: "Tweet ID",
      admin: {
        description: "The ID of the tweet to embed",
      },
    },
  ],
};

// Linked Image Block
export const linkedImageBlock: Block = {
  slug: "linkedImage",
  labels: {
    singular: "Linked Image",
    plural: "Linked Images",
  },
  fields: [
    {
      name: "image",
      type: "upload",
      relationTo: "media",
      required: true,
      label: "Image",
    },
    {
      name: "alt",
      type: "text",
      required: true,
      maxLength: 150,
      label: "Alt Text",
    },
    {
      name: "link",
      type: "group",
      label: "Link",
      fields: [
        {
          name: "label",
          type: "text",
          label: "Label",
        },
        {
          name: "href",
          type: "text",
          required: true,
          label: "URL",
        },
      ],
    },
  ],
};
