import { CollectionConfig } from "payload/types";

const BlogTags: CollectionConfig = {
  slug: "blog-tags",
  admin: {
    useAsTitle: "title",
  },
  access: {
    read: () => true,
  },
  fields: [
    {
      name: "title",
      type: "text",
      required: true,
      label: "Title",
    },
    {
      name: "slug",
      type: "text",
      required: true,
      unique: true,
      label: "Tag's URL-friendly path",
      admin: {
        description: "Auto-generated from title if not provided",
      },
    },
    {
      name: "featuredPosts",
      type: "relationship",
      relationTo: "blog-articles",
      hasMany: true,
      maxRows: 2,
      minRows: 1,
      label: "Featured posts",
      admin: {
        description:
          "Required. These will show on the collections index when this tag is selected.",
      },
    },
  ],
};

export default BlogTags;
