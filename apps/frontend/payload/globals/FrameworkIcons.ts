import { GlobalConfig } from "payload";
import { imageWithAltField } from "../fields/shared/imageWithAlt";
import { slugify } from "@/utils/strings";

export const FrameworkIcons: GlobalConfig = {
  slug: "filter-icon-dictionary",
  dbName: "fid",
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
  hooks: {
    beforeChange: [
      ({ data }) => {
        // Auto-generate IDs from Display Names (slugify and decapitalize)
        if (data?.filters && Array.isArray(data.filters)) {
          data.filters = data.filters.map((filter: any) => {
            // Auto-generate filterId from filterDisplayName (always regenerate to keep in sync)
            if (filter.filterDisplayName) {
              filter.filterId = slugify(filter.filterDisplayName);
            }

            if (filter.filterValues && Array.isArray(filter.filterValues)) {
              filter.filterValues = filter.filterValues.map((fv: any) => {
                // Auto-generate filterValue from filterValueDisplayName (always regenerate to keep in sync)
                if (fv.filterValueDisplayName) {
                  fv.filterValue = slugify(fv.filterValueDisplayName);
                }

                // Remove logo if it's incomplete (missing lightImage or alt)
                if (fv.logo && (!fv.logo.lightImage || !fv.logo.alt)) {
                  const { logo, ...rest } = fv;
                  return rest;
                }
                return fv;
              });
            }
            return filter;
          });
        }
        return data;
      },
    ],
  },
  fields: [
    {
      name: "filters",
      type: "array",
      label: "Filters",
      dbName: "filters",
      fields: [
        {
          name: "filterDisplayName",
          type: "text",
          required: true,
          label: "Name",
          dbName: "name",
          admin: {
            description:
              "Name displayed on the frontend. The ID will be auto-generated from this name.",
          },
        },
        {
          name: "filterId",
          type: "text",
          required: true,
          dbName: "id",
          admin: {
            hidden: true, // Hide ID field - it's auto-generated from Name
          },
        },
        {
          name: "filterValues",
          type: "array",
          required: true,
          label: "Filter Values",
          dbName: "values",
          fields: [
            {
              name: "filterValueDisplayName",
              type: "text",
              required: true,
              label: "Name",
              dbName: "name",
              admin: {
                description:
                  "Name displayed on the frontend. The ID will be auto-generated from this name.",
              },
            },
            {
              name: "filterValue",
              type: "text",
              required: true,
              dbName: "id",
              admin: {
                hidden: true, // Hide ID field - it's auto-generated from Name
              },
            },
            {
              name: "icon",
              type: "text", // Use text field with custom visual picker component
              label: "Icon",
              dbName: "icon",
              admin: {
                description:
                  "Select an icon from the icon library. Use an icon OR an image, not both. Images will take precedence over icons if both are added.",
                components: {
                  Field: "@/payload/components/IconPicker#IconPicker",
                },
              },
            },
            {
              name: "logo",
              type: "group",
              label: "Logo",
              required: false, // Logo is optional - filter values can have icon OR logo
              admin: {
                description:
                  "Logo image (light and dark mode). Images will take precedence over icons if both are added.",
              },
              validate: (value, { data, siblingData }) => {
                // If logo is provided, it must have lightImage and alt
                // If logo is not provided (null/undefined), that's fine
                if (value === null || value === undefined) {
                  return true; // Logo is optional
                }
                if (typeof value === "object") {
                  if (!value.lightImage || !value.alt) {
                    return "Logo must have both light image and alt text if provided";
                  }
                }
                return true;
              },
              fields: [
                {
                  name: "lightImage",
                  type: "upload",
                  relationTo: "media",
                  required: false, // Made optional - validation handled at group level
                  label: "Light Mode Image",
                  admin: {
                    description:
                      "Required image for light mode (only if logo is provided)",
                  },
                },
                {
                  name: "darkImage",
                  type: "upload",
                  relationTo: "media",
                  label: "Dark Mode Image",
                  admin: {
                    description: "Optional image for dark mode",
                  },
                },
                {
                  name: "alt",
                  type: "text",
                  required: false, // Made optional - validation handled at group level
                  maxLength: 150,
                  label: "Descriptive label for screen readers & SEO",
                  admin: {
                    description:
                      "Alt text should be descriptive and concise (under 150 characters) (only if logo is provided)",
                  },
                },
              ],
            },
          ],
        },
      ],
    },
  ],
};

export default FrameworkIcons;
