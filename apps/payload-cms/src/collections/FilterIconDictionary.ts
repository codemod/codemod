import { CollectionConfig } from "payload/types";
import { imageWithAltField } from "../fields/shared/imageWithAlt";

const FilterIconDictionary: CollectionConfig = {
  slug: "filter-icon-dictionary",
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
      defaultValue: "Filter Icons",
    },
    {
      name: "filters",
      type: "array",
      label: "Filters",
      fields: [
        {
          name: "filterId",
          type: "text",
          required: true,
          label: "Name",
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
              label: "Name",
              admin: {
                description: "Match the id of the filter value in the API.",
              },
            },
            {
              name: "icon",
              type: "text",
              label: "Icon",
              admin: {
                description: "Icon name/identifier",
              },
            },
            {
              ...imageWithAltField,
              name: "logo",
              label: "Logo",
              admin: {
                description:
                  "Use an icon OR an image, not both. Images will take precedence over icons if both are added.",
              },
            },
          ],
        },
      ],
    },
  ],
};

export default FilterIconDictionary;
