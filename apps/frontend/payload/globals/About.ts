import { GlobalConfig } from "payload";
import { publishStatusField } from "../fields/shared/publishStatus";
import { imageWithAltField } from "../fields/shared/imageWithAlt";

export const About: GlobalConfig = {
  slug: "about",
  admin: {
    group: "Pages",
  },
  access: {
    read: () => true,
  },
  versions: {
    drafts: true,
  },
  fields: [
    // Page metadata (from definePage helper)
    {
      name: "pathname",
      type: "text",
      defaultValue: "/about",
      admin: {
        readOnly: true,
        description: "Fixed pathname for the about page",
      },
    },
    {
      name: "internalTitle",
      type: "text",
      defaultValue: "About",
      admin: {
        description:
          "This title is only used internally in Payload, it won't be displayed on the website.",
      },
    },
    publishStatusField,
    // SEO handled by @payloadcms/plugin-seo (adds 'meta' field automatically)

    // Hero section (section.hero in Sanity - single object)
    {
      name: "hero",
      type: "group",
      label: "Hero Section",
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
          name: "logoCarousel",
          type: "group",
          label: "Logo Carousel",
          fields: [
            {
              name: "logos",
              type: "array",
              label: "Logos",
              fields: [
                {
                  name: "lightModeImage",
                  type: "upload",
                  relationTo: "media",
                  required: true,
                  label: "Light Mode Image",
                  admin: {
                    description: "Please use a dark logo",
                  },
                },
                {
                  name: "darkModeImage",
                  type: "upload",
                  relationTo: "media",
                  required: true,
                  label: "Dark Mode Image",
                  admin: {
                    description: "Please use a light logo",
                  },
                },
                {
                  name: "alt",
                  type: "text",
                  required: true,
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
    },

    // Paragraph section
    {
      name: "paragraphTitle",
      type: "text",
      required: true,
      label: "Title",
      admin: {
        description: "Title for the paragraph section",
      },
    },
    {
      name: "paragraphContent",
      type: "richText",
      required: true,
      label: "Content",
      admin: {
        description: "Rich text content for the paragraph section",
      },
    },

    // Team section
    {
      name: "teamTitle",
      type: "text",
      required: true,
      label: "Team Section Title",
    },
    {
      name: "teamMembers",
      type: "array",
      label: "Team Members",
      fields: [
        {
          name: "image",
          type: "upload",
          relationTo: "media",
          label: "Image",
          admin: {
            description: "Team member photo",
          },
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
          name: "linkedin",
          type: "text",
          label: "LinkedIn Profile URL",
          admin: {
            description: "Full URL to LinkedIn profile",
          },
        },
        {
          name: "twitter",
          type: "text",
          label: "Twitter Profile URL",
          admin: {
            description: "Full URL to Twitter profile",
          },
        },
        {
          name: "bio",
          type: "richText",
          required: true,
          label: "Bio",
          admin: {
            description: "Team member biography",
          },
        },
        {
          name: "previousCompany",
          type: "text",
          label: "Previous Company",
          admin: {
            description: "Name of previous company",
          },
        },
        {
          ...imageWithAltField,
          name: "previousCompanyLogo",
          label: "Previous Company Logo",
          required: false,
          admin: {
            description:
              "Please, upload logos with transparent background (svg preferred) and in their horizontal variation. Also try and trim vertical margins as much as possible.",
          },
        },
      ],
    },

    // Companies section - simple structure with title, subtitle, and logo carousel
    {
      name: "companies",
      type: "group",
      label: "Companies Section",
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
          name: "logoCarousel",
          type: "group",
          label: "Logo Carousel",
          fields: [
            {
              name: "logos",
              type: "array",
              label: "Logos",
              fields: [
                {
                  name: "lightModeImage",
                  type: "upload",
                  relationTo: "media",
                  required: true,
                  label: "Light Mode Image",
                  admin: {
                    description: "Please use a dark logo",
                  },
                },
                {
                  name: "darkModeImage",
                  type: "upload",
                  relationTo: "media",
                  required: true,
                  label: "Dark Mode Image",
                  admin: {
                    description: "Please use a light logo",
                  },
                },
                {
                  name: "alt",
                  type: "text",
                  required: true,
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
    },

    // Investors section
    {
      name: "investorsTitle",
      type: "text",
      required: true,
      label: "Investors Section Title",
    },
    {
      name: "investorsSubtitle",
      type: "richText",
      required: true,
      label: "Investors Section Subtitle",
      admin: {
        description: "Rich text subtitle for the investors section",
      },
    },
    {
      name: "investors",
      type: "array",
      label: "Investors",
      fields: [
        {
          name: "image",
          type: "upload",
          relationTo: "media",
          label: "Image",
          admin: {
            description: "Investor photo",
          },
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
          ...imageWithAltField,
          name: "companyLogo",
          label: "Company Logo",
          required: false,
          admin: {
            description: "Company logo with light and dark mode support",
          },
        },
      ],
    },

    // Page CTA (optional)
    {
      name: "pageCta",
      type: "relationship",
      relationTo: "ctas",
      label: "Page CTA (Optional)",
      admin: {
        description:
          "Call to action for a page. This is placed at the bottom of the page before the footer.",
      },
    },
  ],
};

export default About;
