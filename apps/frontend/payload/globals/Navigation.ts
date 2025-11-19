import { GlobalConfig } from "payload";
import { linkField } from "../fields/shared/link";

export const Navigation: GlobalConfig = {
  slug: "navigation",
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
      defaultValue: "Navigation",
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
      fields: [linkField],
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
      fields: [linkField],
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
            condition: (_, siblingData) => siblingData?.enabled === true,
            description: "Required when announcement bar is enabled",
          },
        },
      ],
    },
  ],
};

export default Navigation;
