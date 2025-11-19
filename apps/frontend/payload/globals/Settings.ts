import { GlobalConfig } from "payload";
import { imageWithAltField } from "../fields/shared/imageWithAlt";

export const Settings: GlobalConfig = {
  slug: "settings",
  admin: {
    group: "Site Configuration",
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
      defaultValue: "Settings",
      admin: {
        hidden: true,
      },
    },
    {
      ...imageWithAltField,
      name: "fallbackOgImage",
      label: "Fallback sharing image",
      required: true,
      admin: {
        description:
          "Will be used as the sharing image of all pages that don't define a custom one in their SEO fields.",
      },
    },
    {
      name: "redirects",
      type: "array",
      label: "Redirects",
      fields: [
        {
          name: "source",
          type: "text",
          required: true,
          label: "Source",
          admin: {
            description: "The source URL path (e.g., /old-page)",
          },
        },
        {
          name: "destination",
          type: "text",
          required: true,
          label: "Destination",
          admin: {
            description: "The destination URL path (e.g., /new-page)",
          },
        },
        {
          name: "permanent",
          type: "checkbox",
          defaultValue: true,
          label: "Permanent redirect",
          admin: {
            description:
              "Turn this off if the redirect is temporary and you intend on reverting it in the near future.",
          },
        },
      ],
    },
  ],
};

export default Settings;
