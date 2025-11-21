import type { Block } from "payload";
import {
  lexicalEditor,
  BlocksFeature,
  CodeBlock,
  EXPERIMENTAL_TableFeature,
} from "@payloadcms/richtext-lexical";

// Note: CodeBlock is built-in from @payloadcms/richtext-lexical
// We use CodeBlock() instead of a custom code snippet block

// Note: Payload has built-in image support in Lexical
// We only need the custom "Linked Image" block below

// Linked Image Block
// Using flat fields to avoid Payload UI state conflicts with multiple upload fields
export const linkedImageBlock: Block = {
  slug: "linked-image",
  labels: {
    singular: "Linked Image",
    plural: "Linked Images",
  },
  fields: [
    {
      name: "lightImage",
      type: "upload",
      relationTo: "media",
      label: "Light Mode Image",
      admin: {
        description:
          "Image for light mode (optional if dark mode image is provided)",
      },
    },
    {
      name: "darkImage",
      type: "upload",
      relationTo: "media",
      label: "Dark Mode Image",
      admin: {
        description:
          "Image for dark mode (optional if light mode image is provided)",
      },
    },
    {
      name: "alt",
      type: "text",
      label: "Alt Text",
      validate: (value: string | null | undefined, { siblingData }: any) => {
        const hasLight = !!siblingData?.lightImage;
        const hasDark = !!siblingData?.darkImage;
        if ((hasLight || hasDark) && (!value || value.trim() === "")) {
          return "Alt text is required when an image is provided";
        }
        return true;
      },
    },
    {
      name: "linkLabel",
      type: "text",
      label: "Link Label",
    },
    {
      name: "linkUrl",
      type: "text",
      label: "Link URL",
      validate: (value: string | null | undefined, { siblingData }: any) => {
        if (siblingData.linkLabel && !value) {
          return "Link URL is required when Link Label is provided";
        }
        if (value && !value.match(/^(https?:\/\/|\/)/)) {
          return "Link URL must start with http://, https://, or /";
        }
        return true;
      },
    },
    {
      name: "caption",
      type: "text",
      label: "Caption",
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

// Admonition Block (callout/alert block)
// Note: In Sanity, admonitions were inline annotations, but we convert them to blocks in Payload
export const admonitionBlock: Block = {
  slug: "admonition",
  labels: {
    singular: "Admonition",
    plural: "Admonitions",
  },
  fields: [
    {
      name: "variant",
      type: "select",
      required: true,
      label: "Variant",
      defaultValue: "success",
      options: [
        { label: "Success", value: "success" },
        { label: "Info", value: "info" },
        { label: "Warning", value: "warning" },
        { label: "Error", value: "error" },
      ],
    },
    {
      name: "title",
      type: "text",
      label: "Title",
      defaultValue: "Tip",
      admin: {
        description: "Title shown at the top of the admonition",
      },
    },
    {
      name: "icon",
      type: "text",
      label: "Icon",
      defaultValue: "standard",
      admin: {
        description: "Icon name",
        components: {
          Field: "@/payload/components/IconPicker#IconPicker",
        },
      },
    },
    {
      name: "content",
      type: "richText",
      required: true,
      label: "Content",
      admin: {
        description: "The content of the admonition",
      },
      editor: lexicalEditor({
        features: ({ defaultFeatures }) => [
          ...defaultFeatures,
          EXPERIMENTAL_TableFeature(),
          BlocksFeature({
            blocks: [
              CodeBlock(),
              ...richTextBlocksWithoutCollapsibleOrAdmonition,
            ],
          }),
        ],
      }),
    },
  ],
};

// Quote Block (custom - different from built-in blockquote)
// Note: Payload has a built-in blockquote feature, but this is a custom "Quote Block"
// with image, author, etc. - similar to Sanity's quoteBlock
// Using flat fields to avoid Payload UI state conflicts with multiple upload fields
// Field order matches Sanity structure: image, quote, authorName, authorPosition, authorImage
export const quoteBlock: Block = {
  slug: "quote-block",
  labels: {
    singular: "Quote Block",
    plural: "Quote Blocks",
  },
  fields: [
    {
      name: "imageLight",
      type: "upload",
      relationTo: "media",
      label: "Image (Light Mode)",
      admin: {
        description:
          "Quote image for light mode (optional if dark mode image is provided)",
      },
    },
    {
      name: "imageDark",
      type: "upload",
      relationTo: "media",
      label: "Image (Dark Mode)",
      admin: {
        description:
          "Quote image for dark mode (optional if light mode image is provided)",
      },
    },
    {
      name: "imageAlt",
      type: "text",
      label: "Image Alt Text",
      validate: (value: string | null | undefined, { siblingData }: any) => {
        const hasLight = !!siblingData?.imageLight;
        const hasDark = !!siblingData?.imageDark;
        if ((hasLight || hasDark) && (!value || value.trim() === "")) {
          return "Alt text is required when an image is provided";
        }
        return true;
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
      label: "Author",
    },
    {
      name: "authorPosition",
      type: "text",
      label: "Author Position",
    },
    {
      name: "authorImageLight",
      type: "upload",
      relationTo: "media",
      label: "Author Image (Light Mode)",
      admin: {
        description:
          "Author image for light mode (optional if dark mode image is provided)",
      },
    },
    {
      name: "authorImageDark",
      type: "upload",
      relationTo: "media",
      label: "Author Image (Dark Mode)",
      admin: {
        description:
          "Author image for dark mode (optional if light mode image is provided)",
      },
    },
    {
      name: "authorImageAlt",
      type: "text",
      label: "Author Image Alt Text",
      validate: (value: string | null | undefined, { siblingData }: any) => {
        const hasLight = !!siblingData?.authorImageLight;
        const hasDark = !!siblingData?.authorImageDark;
        if ((hasLight || hasDark) && (!value || value.trim() === "")) {
          return "Alt text is required when an image is provided";
        }
        return true;
      },
    },
  ],
};

// Note: Table functionality is provided by EXPERIMENTAL_TableFeature (built-in)
// Removed custom tableBlock in favor of the experimental built-in feature

// Blocks without collapsible or admonition (for use in nested richText fields)
// Note: CodeBlock is added separately in the editor config (it's a built-in feature, not a block)
// Note: Built-in image support is available in Lexical, we only need linked-image
// Note: Table functionality is provided by EXPERIMENTAL_TableFeature (built-in)
const richTextBlocksWithoutCollapsibleOrAdmonition: Block[] = [
  linkedImageBlock, // Custom - only this image variant is needed
  muxVideoWithCaptionBlock, // Custom - no Payload plugin for Mux (Sanity used mux.video plugin)
  youtubeVideoBlock, // Custom - no built-in YouTube embed
  twitterEmbedBlock, // Custom - no built-in Twitter embed
  quoteBlock, // Custom - different from built-in blockquote
];

// All blocks including collapsible and admonition (for top-level richText fields)
const richTextBlocksWithoutCollapsible: Block[] = [
  ...richTextBlocksWithoutCollapsibleOrAdmonition,
  admonitionBlock, // Custom - callout/alert blocks (converted from Sanity inline annotations)
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
            blocks: [
              CodeBlock(),
              ...richTextBlocksWithoutCollapsibleOrAdmonition,
            ],
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
