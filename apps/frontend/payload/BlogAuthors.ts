import { CollectionConfig } from "payload";
import { imageWithAltField } from "./fields/shared/imageWithAlt";

export const BlogAuthors: CollectionConfig = {
  slug: "blog-authors",
  admin: {
    group: "Blog",
    useAsTitle: "name",
  },
  access: {
    read: () => true,
  },
  fields: [
    {
      name: "name",
      type: "text",
      required: true,
      label: "Name",
    },
    {
      name: "details",
      type: "textarea",
      label: "Additional info about the author",
      admin: {
        description: "Optional additional information about the author",
      },
    },
    {
      name: "socialUrl",
      type: "text",
      label: "Social URL",
      admin: {
        description: "URL to author's social media profile",
      },
    },
    {
      ...imageWithAltField,
      name: "image",
      label: "Author Image",
      admin: {
        description: "Author profile image with light and dark mode support",
      },
    },
  ],
};

export default BlogAuthors;
