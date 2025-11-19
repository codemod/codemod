import { GlobalConfig } from "payload";

export const NotFound: GlobalConfig = {
  slug: "not-found",
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
    {
      name: "pathname",
      type: "text",
      defaultValue: "/404",
      admin: {
        readOnly: true,
        description: "Fixed pathname for the 404 page",
      },
    },
    {
      name: "internalTitle",
      type: "text",
      defaultValue: "Not Found",
      admin: {
        description: "Internal title for admin use only",
      },
    },
    {
      name: "title",
      type: "text",
      label: "Title",
    },
    {
      name: "description",
      type: "textarea",
      label: "Description",
      admin: {
        rows: 3,
      },
    },
    {
      name: "heroCta",
      type: "group",
      label: "Hero CTA",
      admin: {
        description: "Call to action button for the hero section",
      },
      fields: [
        {
          name: "label",
          type: "text",
          label: "Button label",
          admin: {
            description: "Optional button label text",
          },
        },
        {
          name: "link",
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
      name: "footerCta",
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

export default NotFound;
