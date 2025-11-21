import { GlobalConfig } from "payload";
import { linkField } from "../fields/shared/link";
import { imageWithAltField } from "../fields/shared/imageWithAlt";

export const Footer: GlobalConfig = {
  slug: "footer",
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
      defaultValue: "Footer",
      admin: {
        hidden: true,
      },
    },
    {
      name: "footerText",
      type: "richText",
      required: false,
      label: "Footer Text",
      admin: {
        description:
          "Footer text content. Note: Rich text from Sanity needs manual migration from PortableText to Lexical.",
      },
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
          name: "icon",
          type: "text",
          label: "Icon",
          admin: {
            description: "Select an icon from the icon library",
            components: {
              Field: "@/payload/components/IconPicker#IconPicker",
            },
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
          label: "Submenu",
          options: [
            { label: "Product", value: "product" },
            { label: "Company", value: "company" },
            { label: "Legal", value: "legal" },
          ],
        },
        {
          name: "links",
          type: "array",
          label: "Footer Links",
          fields: [linkField],
        },
      ],
    },
  ],
};

export default Footer;
