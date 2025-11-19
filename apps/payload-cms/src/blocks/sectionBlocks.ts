import { Block } from "payload/types";
import { imageWithAltField } from "../fields/shared/imageWithAlt";
import { linkField } from "../fields/shared/link";

// Hero Section Block
export const heroSectionBlock: Block = {
  slug: "section-hero",
  labels: {
    singular: "Hero Section",
    plural: "Hero Sections",
  },
  fields: [
    {
      name: "title",
      type: "text",
      required: true,
      maxLength: 80,
      label: "Title",
      admin: {
        description: "Max 80 chars",
      },
    },
    {
      name: "subtitle",
      type: "textarea",
      maxLength: 250,
      label: "Subtitle",
      admin: {
        description: "Max 250 chars",
      },
    },
    {
      name: "ctas",
      type: "array",
      maxRows: 2,
      label: "CTAs",
      fields: [
        {
          name: "label",
          type: "text",
          label: "Button label",
        },
        {
          name: "link",
          type: "text",
          required: true,
          label: "Link URL",
        },
      ],
    },
    {
      name: "logoCarousel",
      type: "group",
      label: "Logo Carousel",
      fields: [
        {
          name: "title",
          type: "text",
          label: "Title",
        },
        {
          name: "logos",
          type: "array",
          label: "Logos",
          fields: [
            {
              name: "lightModeImage",
              type: "upload",
              relationTo: "media",
              label: "Light Mode Image",
            },
            {
              name: "darkModeImage",
              type: "upload",
              relationTo: "media",
              label: "Dark Mode Image",
            },
            {
              name: "alt",
              type: "text",
              label: "Alt Text",
            },
            {
              name: "link",
              type: "text",
              required: true,
              label: "Link",
              admin: {
                description: "e.g. https://example.com or /about-page",
              },
            },
          ],
        },
      ],
    },
  ],
};

// Features Section Block
export const featuresSectionBlock: Block = {
  slug: "section-features",
  labels: {
    singular: "Features Section",
    plural: "Features Sections",
  },
  fields: [
    {
      name: "features",
      type: "array",
      minRows: 2,
      maxRows: 5,
      label: "Features",
      admin: {
        description:
          "The third feature will be displayed as a large card. The rest will be small cards.",
      },
      fields: [
        {
          name: "background",
          type: "group",
          label: "Background",
          fields: [
            {
              name: "light",
              type: "group",
              label: "Light Version",
              fields: [
                {
                  name: "type",
                  type: "select",
                  required: true,
                  options: [
                    { label: "Image", value: "image" },
                    { label: "Video", value: "video" },
                  ],
                  label: "Type",
                },
                {
                  name: "asset",
                  type: "text",
                  label: "Asset (Mux Playback ID)",
                  admin: {
                    condition: (data, siblingData) =>
                      siblingData?.type === "video",
                  },
                },
                {
                  name: "image",
                  type: "upload",
                  relationTo: "media",
                  label: "Image",
                  admin: {
                    condition: (data, siblingData) =>
                      siblingData?.type === "image",
                  },
                },
              ],
            },
            {
              name: "dark",
              type: "group",
              label: "Dark Version",
              fields: [
                {
                  name: "type",
                  type: "select",
                  required: true,
                  options: [
                    { label: "Image", value: "image" },
                    { label: "Video", value: "video" },
                  ],
                  label: "Type",
                },
                {
                  name: "asset",
                  type: "text",
                  label: "Asset (Mux Playback ID)",
                  admin: {
                    condition: (data, siblingData) =>
                      siblingData?.type === "video",
                  },
                },
                {
                  name: "image",
                  type: "upload",
                  relationTo: "media",
                  label: "Image",
                  admin: {
                    condition: (data, siblingData) =>
                      siblingData?.type === "image",
                  },
                },
              ],
            },
          ],
        },
        {
          name: "tag",
          type: "text",
          label: "Tag",
        },
        {
          name: "title",
          type: "text",
          required: true,
          label: "Title",
        },
        {
          name: "description",
          type: "textarea",
          label: "Description",
        },
        {
          name: "snippet",
          type: "text",
          label: "Code Snippet",
          admin: {
            description: "A code snippet to be displayed in the feature card",
          },
        },
        {
          name: "toastText",
          type: "text",
          label: "Toast Text",
          admin: {
            description:
              "Text to display in the confirmation toast. Defaults to 'Copied command to clipboard'",
          },
        },
        {
          ...linkField,
          name: "cta",
          label: "Call to action",
        },
      ],
    },
  ],
};

// Registry Section Block
export const registrySectionBlock: Block = {
  slug: "section-registry",
  labels: {
    singular: "Registry Section",
    plural: "Registry Sections",
  },
  fields: [
    {
      name: "title",
      type: "text",
      label: "Title",
    },
    {
      name: "subtitle",
      type: "textarea",
      label: "Subtitle",
    },
    {
      name: "initialAutomationSlugs",
      type: "array",
      maxRows: 4,
      label: "Initial Automations",
      admin: {
        description: "List of automation slugs to display in the registry",
      },
      fields: [
        {
          name: "slug",
          type: "text",
          label: "Automation Slug",
        },
      ],
    },
    {
      name: "searchPlaceholder",
      type: "text",
      label: "Search placeholder",
      admin: {
        description:
          'Placeholder text for the search input. Defaults to "Search for Codemods"',
      },
    },
    {
      name: "ctaLabel",
      type: "text",
      label: "CTA label",
      admin: {
        description:
          'Label for the CTA button. Defaults to "View all Codemods".',
      },
    },
    {
      name: "automationFilter",
      type: "text",
      label: "Automation filter",
      admin: {
        description: "Select the automation filter to use for this section",
      },
    },
  ],
};

// Testimonials Section Block
export const testimonialsSectionBlock: Block = {
  slug: "section-testimonials",
  labels: {
    singular: "Testimonials",
    plural: "Testimonials",
  },
  fields: [
    {
      name: "title",
      type: "text",
      required: true,
      label: "Title",
    },
    {
      name: "paragraph",
      type: "richText",
      label: "Paragraph",
    },
    {
      name: "items",
      type: "array",
      label: "Testimonials",
      fields: [
        {
          name: "companyLogoLight",
          type: "upload",
          relationTo: "media",
          required: true,
          label: "Company Logo Light Mode",
        },
        {
          name: "companyLogoDark",
          type: "upload",
          relationTo: "media",
          required: true,
          label: "Company Logo Dark Mode",
        },
        {
          name: "name",
          type: "text",
          required: true,
          label: "Name",
        },
        {
          name: "role",
          type: "text",
          required: true,
          label: "Role",
        },
        {
          name: "image",
          type: "upload",
          relationTo: "media",
          required: true,
          label: "Image",
        },
        {
          name: "quote",
          type: "richText",
          required: true,
          label: "Quote",
        },
      ],
    },
  ],
};

// Full Width Media Section Block
export const fullWidthMediaSectionBlock: Block = {
  slug: "section-full-width-media",
  labels: {
    singular: "Full Width Media",
    plural: "Full Width Media",
  },
  fields: [
    {
      name: "title",
      type: "text",
      label: "Title",
    },
    {
      name: "subtitle",
      type: "textarea",
      label: "Subtitle",
    },
    {
      name: "mediaTabs",
      type: "array",
      label: "Media Tabs",
      fields: [
        {
          name: "tabTitle",
          type: "text",
          label: "Tab Title",
          admin: {
            description: "The title of the tab used to switch items",
          },
        },
        {
          name: "mediaItem",
          type: "array",
          maxRows: 1,
          minRows: 1,
          label: "Media Item",
          admin: {
            description: "The media item to display in the tab. Max 1 item.",
          },
          fields: [
            {
              name: "type",
              type: "select",
              required: true,
              options: [
                { label: "Mux Video", value: "muxVideo" },
                { label: "Image", value: "image" },
              ],
              label: "Media Type",
            },
            {
              name: "muxVideo",
              type: "group",
              label: "Mux Video",
              admin: {
                condition: (data, siblingData) =>
                  siblingData?.type === "muxVideo",
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
                },
                {
                  name: "loop",
                  type: "checkbox",
                  label: "Loop",
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
            },
            {
              ...imageWithAltField,
              name: "image",
              label: "Image",
              admin: {
                condition: (data, siblingData) => siblingData?.type === "image",
              },
            },
          ],
        },
      ],
    },
  ],
};
