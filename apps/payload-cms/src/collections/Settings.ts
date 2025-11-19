import { CollectionConfig } from "payload/types";

const Settings: CollectionConfig = {
  slug: "settings",
  admin: {
    useAsTitle: "title",
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
      label: "Title",
      admin: {
        hidden: true,
      },
    },
    {
      name: "fallbackOgImage",
      type: "upload",
      relationTo: "media",
      required: true,
      label: "Fallback sharing image",
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
        },
        {
          name: "destination",
          type: "text",
          required: true,
          label: "Destination",
        },
        {
          name: "permanent",
          type: "checkbox",
          defaultValue: true,
          label: "Permanent",
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
