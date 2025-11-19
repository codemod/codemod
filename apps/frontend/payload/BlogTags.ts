import { CollectionConfig } from "payload";

export const BlogTags: CollectionConfig = {
  slug: "blog-tags",
  admin: {
    group: "Blog",
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
        description:
          'URL-friendly version of the title (e.g., "getting-started")',
      },
    },
    {
      name: "featuredPosts",
      type: "relationship",
      relationTo: "blog-posts",
      hasMany: true,
      minRows: 1,
      maxRows: 2,
      label: "Featured posts",
      admin: {
        description:
          'Required. These will show on the collections index when this tag is selected. Select posts with postType = "article"',
      },
    },
  ],
};

export default BlogTags;
