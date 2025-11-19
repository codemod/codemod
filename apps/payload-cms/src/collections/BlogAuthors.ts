import { CollectionConfig } from "payload/types";
import { imageWithAltField } from "../fields/shared/imageWithAlt";

const BlogAuthors: CollectionConfig = {
  slug: "blog-authors",
  admin: {
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
      type: "text",
      label: "Additional info about the author",
    },
    {
      name: "socialUrl",
      type: "text",
      label: "Social URL",
    },
    {
      ...imageWithAltField,
      name: "image",
      label: "Author Image",
    },
  ],
};

export default BlogAuthors;
