import { GlobalConfig } from "payload";
import { imageWithAltField } from "../fields/shared/imageWithAlt";

export const FrameworkIcons: GlobalConfig = {
  slug: "filter-icon-dictionary",
  admin: {
    group: "Globals",
    label: "Framework Icons",
  },
  access: {
    read: () => true,
  },
  versions: {
    drafts: true,
  },
  fields: [
    {
      name: "filters",
      type: "array",
      label: "Filters",
      fields: [
        {
          name: "filterId",
          type: "text",
          required: true,
          label: "Filter ID",
          admin: {
            description: "Match the id of the filter in the API.",
          },
        },
        {
          name: "filterValues",
          type: "array",
          required: true,
          label: "Filter Values",
          fields: [
            {
              name: "filterValue",
              type: "text",
              required: true,
              label: "Filter Value",
              admin: {
                description: "Match the id of the filter value in the API.",
              },
            },
            {
              name: "icon",
              type: "text",
              label: "Icon",
              admin: {
                description:
                  "Icon name/identifier. Use an icon OR an image, not both. Images will take precedence over icons if both are added.",
              },
            },
            {
              ...imageWithAltField,
              name: "logo",
              label: "Logo",
              admin: {
                description:
                  "Logo image (light and dark mode). Images will take precedence over icons if both are added.",
              },
            },
          ],
        },
      ],
    },
  ],
};

export default FrameworkIcons;
