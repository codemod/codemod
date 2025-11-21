import type { Field } from "payload";

export const imageWithAltField: Field = {
  name: "imageWithAlt",
  type: "group",
  label: "Image with Light and Dark Mode Support",
  required: false, // Make the group optional by default (can be overridden when used)
  // Flexible validation: Allow just lightImage, just darkImage, or both
  // Alt text is required if at least one image is provided
  // Only validate on save, not during editing to allow both fields to be populated
  validate: (value, { data, siblingData, operation }) => {
    // During create/update operations, validate
    if (operation === "create" || operation === "update") {
      // If image group is not provided (null/undefined), that's fine - it's optional
      if (value === null || value === undefined) {
        return true;
      }

      // If image object exists, validate it
      if (typeof value === "object" && value !== null) {
        const imageValue = value as {
          lightImage?: any;
          darkImage?: any;
          alt?: string;
        };
        const hasLightImage = !!imageValue.lightImage;
        const hasDarkImage = !!imageValue.darkImage;
        const hasAlt = imageValue.alt && imageValue.alt.trim() !== "";

        // At least one image must be provided
        if (!hasLightImage && !hasDarkImage) {
          return "At least one image (light mode or dark mode) must be provided";
        }

        // Alt text is required if any image is provided
        if (!hasAlt) {
          return "Alt text is required when an image is provided (for accessibility and SEO)";
        }
      }
    }

    // During other operations (like reading), always pass
    return true;
  },
  fields: [
    {
      name: "lightImage",
      type: "upload",
      relationTo: "media",
      required: false,
      label: "Light Mode Image",
      admin: {
        description:
          "Image displayed in light mode. You can provide just this, just dark mode, or both.",
        condition: () => true, // Always show - no conditions
      },
    },
    {
      name: "darkImage",
      type: "upload",
      relationTo: "media",
      required: false,
      label: "Dark Mode Image",
      admin: {
        description:
          "Image displayed in dark mode. You can provide just this, just light mode, or both.",
        condition: () => true, // Always show - no conditions
      },
    },
    {
      name: "alt",
      type: "text",
      required: false, // Validation handled at group level
      maxLength: 150,
      label: "Alt Text",
      admin: {
        description:
          "Descriptive text for screen readers and SEO (required if any image is provided, max 150 characters)",
        condition: () => true, // Always show - no conditions
      },
    },
  ],
};
