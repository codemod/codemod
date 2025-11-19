import { CollectionConfig } from "payload/types";
import { linkField } from "../fields/shared/link";

const Footer: CollectionConfig = {
  slug: "footer",
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
      name: "footerText",
      type: "richText",
      required: true,
      label: "Footer Text",
    },
    {
      name: "socialLinks",
      type: "array",
      label: "Social links",
      fields: [
        {
          ...linkField,
          name: "link",
          label: "Link",
        },
        {
          name: "logo",
          type: "text",
          label: "Logo Name",
          admin: {
            description: "Icon/logo identifier",
          },
        },
      ],
    },
    {
      name: "footerNavigationItems",
      type: "array",
      label: "Footer Navigation items",
      fields: [
        {
          name: "submenu",
          type: "select",
          required: true,
          options: [
            { label: "Product", value: "product" },
            { label: "Company", value: "company" },
            { label: "Legal", value: "legal" },
          ],
          label: "Submenu",
        },
        {
          name: "links",
          type: "array",
          label: "Footer Links",
          fields: [
            {
              ...linkField,
            },
          ],
        },
      ],
    },
  ],
};

export default Footer;
