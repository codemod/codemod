import type { Block } from "payload";
import {
  lexicalEditor,
  BlocksFeature,
  CodeBlock,
  EXPERIMENTAL_TableFeature,
} from "@payloadcms/richtext-lexical";
import { imageWithAltField } from "../fields/shared/imageWithAlt";

// Note: CodeBlock is built-in from @payloadcms/richtext-lexical
// We use CodeBlock() instead of a custom code snippet block

// Note: Payload has built-in image support in Lexical
// We only need the custom "Linked Image" block below

// Linked Image Block
export const linkedImageBlock: Block = {
  slug: "linked-image",
  labels: {
    singular: "Linked Image",
    plural: "Linked Images",
  },
  fields: [
    {
      ...imageWithAltField,
      name: "image",
      label: "Image",
      required: true,
    },
    {
      name: "link",
      type: "group",
      label: "Link",
      fields: [
        {
          name: "label",
          type: "text",
          label: "Link Label",
          admin: {
            description: "Optional label for the link",
          },
        },
        {
          name: "href",
          type: "text",
          required: true,
          label: "URL",
          admin: {
            description: "e.g. https://example.com or /about-page",
          },
        },
      ],
    },
    {
      name: "caption",
      type: "text",
      label: "Caption",
      admin: {
        description: "Optional caption for the image",
      },
    },
  ],
};

// Mux Video Block (with caption)
export const muxVideoWithCaptionBlock: Block = {
  slug: "mux-video",
  labels: {
    singular: "Mux Video",
    plural: "Mux Videos",
  },
  fields: [
    {
      name: "hasControls",
      type: "checkbox",
      label: "Show video controls",
      defaultValue: true,
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
      required: true,
      label: "Light Mode Video (Mux Playback ID)",
      admin: {
        description: "Mux playback ID for light mode video",
      },
    },
    {
      name: "darkVideo",
      type: "text",
      label: "Dark Mode Video (Mux Playback ID)",
      admin: {
        description: "Optional Mux playback ID for dark mode video",
      },
    },
    {
      name: "caption",
      type: "text",
      label: "Caption",
      admin: {
        description: "Optional caption for the video",
      },
    },
  ],
};

// YouTube Video Block
export const youtubeVideoBlock: Block = {
  slug: "youtube-video",
  labels: {
    singular: "YouTube Video",
    plural: "YouTube Videos",
  },
  fields: [
    {
      name: "youtubeUrl",
      type: "text",
      required: true,
      label: "YouTube URL",
      admin: {
        description:
          "Link to YouTube embedded video (e.g., https://www.youtube.com/watch?v=...)",
      },
    },
    {
      name: "caption",
      type: "text",
      label: "Caption",
      admin: {
        description: "Optional caption for the video",
      },
    },
  ],
};

// Twitter Embed Block
export const twitterEmbedBlock: Block = {
  slug: "twitter-embed",
  labels: {
    singular: "Twitter Embed",
    plural: "Twitter Embeds",
  },
  fields: [
    {
      name: "url",
      type: "text",
      required: true,
      label: "Tweet URL",
      admin: {
        description:
          "Tweet ID, Tweet URL or Embed code. e.g. https://twitter.com/vercel/status/1355559917686780416",
      },
    },
  ],
};

// Quote Block (custom - different from built-in blockquote)
// Note: Payload has a built-in blockquote feature, but this is a custom "Quote Block"
// with image, author, etc. - similar to Sanity's quoteBlock
export const quoteBlock: Block = {
  slug: "quote-block",
  labels: {
    singular: "Quote Block",
    plural: "Quote Blocks",
  },
  fields: [
    {
      ...imageWithAltField,
      name: "image",
      label: "Image",
      admin: {
        description: "Optional image for the quote",
      },
    },
    {
      name: "quote",
      type: "textarea",
      required: true,
      label: "Quote",
      admin: {
        rows: 4,
        description: "The quote text",
      },
    },
    {
      name: "authorName",
      type: "text",
      required: true,
      label: "Author Name",
    },
    {
      name: "authorPosition",
      type: "text",
      label: "Author Position",
      admin: {
        description: "Optional position/title of the author",
      },
    },
    {
      ...imageWithAltField,
      name: "authorImage",
      label: "Author Image",
      admin: {
        description: "Optional author profile image",
      },
    },
  ],
};

// Note: Table functionality is provided by EXPERIMENTAL_TableFeature (built-in)
// Removed custom tableBlock in favor of the experimental built-in feature

// Blocks without collapsible (defined first for use in collapsible content)
// Note: CodeBlock is added separately in the editor config (it's a built-in feature, not a block)
// Note: Built-in image support is available in Lexical, we only need linked-image
// Note: Table functionality is provided by EXPERIMENTAL_TableFeature (built-in)
const richTextBlocksWithoutCollapsible: Block[] = [
  linkedImageBlock, // Custom - only this image variant is needed
  muxVideoWithCaptionBlock, // Custom - no Payload plugin for Mux (Sanity used mux.video plugin)
  youtubeVideoBlock, // Custom - no built-in YouTube embed
  twitterEmbedBlock, // Custom - no built-in Twitter embed
  quoteBlock, // Custom - different from built-in blockquote
];

// Collapsible Block
// Note: The nested richText field should exclude collapsible blocks to prevent infinite nesting
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
      admin: {
        description: "Title for the collapsible section",
      },
    },
    {
      name: "content",
      type: "richText",
      required: true,
      label: "Content",
      admin: {
        description:
          "Content that will be shown when expanded (rich text without collapsible blocks)",
      },
      // Exclude collapsible blocks from nested content to prevent infinite nesting
      editor: lexicalEditor({
        features: ({ defaultFeatures }) => [
          ...defaultFeatures,
          EXPERIMENTAL_TableFeature(), // Built-in experimental table feature
          BlocksFeature({
            blocks: [CodeBlock(), ...richTextBlocksWithoutCollapsible],
          }),
        ],
      }),
    },
  ],
};

// Export all blocks for use in rich text fields
export const richTextBlocks: Block[] = [
  ...richTextBlocksWithoutCollapsible,
  collapsibleBlock,
];
