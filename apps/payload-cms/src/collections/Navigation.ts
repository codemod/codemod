import { CollectionConfig } from "payload/types";
import { linkField } from "../fields/shared/link";

const Navigation: CollectionConfig = {
  slug: "navigation",
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
      label: "Internal Title",
      admin: {
        hidden: true,
      },
    },
    {
      name: "navigationItems",
      type: "array",
      maxRows: 6,
      label: "Navigation items",
      admin: {
        description:
          "Add the items you want to appear in the main navigation (Max 6 items)",
      },
      fields: [
        {
          ...linkField,
        },
      ],
    },
    {
      name: "navigationCtas",
      type: "array",
      maxRows: 2,
      label: "Navigation CTA items",
      admin: {
        description:
          "Desktop: Top right corner, Mobile: Bottom of the menu. 1st link will be the primary CTA. Max 2 items.",
      },
      fields: [
        {
          ...linkField,
        },
      ],
    },
    {
      name: "announcementBar",
      type: "group",
      label: "Announcement Bar",
      fields: [
        {
          name: "enabled",
          type: "checkbox",
          label: "Enable",
        },
        {
          name: "dismissable",
          type: "checkbox",
          label: "Dismissable",
        },
        {
          name: "message",
          type: "richText",
          required: true,
          label: "Message",
          admin: {
            condition: (data) => data?.enabled === true,
          },
        },
      ],
    },
  ],
};

export default Navigation;
