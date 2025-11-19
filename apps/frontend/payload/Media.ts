import { CollectionConfig } from "payload";

export const Media: CollectionConfig = {
  slug: "media",
  admin: {
    group: "Globals",
  },
  access: {
    read: () => true,
  },
  upload: true,
  fields: [
    {
      name: "alt",
      type: "text",
      label: "Alt Text",
      admin: {
        description: "Descriptive label for screen readers & SEO",
      },
    },
  ],
};

export default Media;
