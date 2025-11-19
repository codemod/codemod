import { GlobalConfig } from "payload";
import { publishStatusField } from "../fields/shared/publishStatus";

export const BlogIndex: GlobalConfig = {
  slug: "blog-index",
  admin: {
    group: "Blog",
  },
  access: {
    read: () => true,
  },
  versions: {
    drafts: true,
  },
  fields: [
    {
      name: "pathname",
      type: "text",
      defaultValue: "/blog",
      admin: {
        readOnly: true,
        description: "Fixed pathname for the blog index page",
      },
    },
    {
      name: "internalTitle",
      type: "text",
      defaultValue: "Blog Index",
      admin: {
        description: "Internal title for admin use only",
      },
    },
    publishStatusField,
    // SEO handled by @payloadcms/plugin-seo (adds 'meta' field automatically)

    // Blog Index-specific fields
    {
      name: "title",
      type: "text",
      label: "Title",
      admin: {
        description: "Optional, shows in collection index",
      },
    },
    {
      name: "collectionTitle",
      type: "text",
      label: "Collection Title",
      admin: {
        description: "Optional, shows above collection",
      },
    },
    {
      name: "featuredPosts",
      type: "relationship",
      relationTo: "blog-posts",
      hasMany: true,
      minRows: 1,
      maxRows: 2,
      label: "Featured Posts",
      admin: {
        description:
          'Required. Featured blog articles (1-2). Select posts with postType = "article"',
      },
    },
    {
      name: "featuredCustomerStories",
      type: "relationship",
      relationTo: "blog-posts",
      hasMany: true,
      minRows: 1,
      maxRows: 2,
      label: "Featured Customer Stories",
      admin: {
        description:
          'Required. Featured customer stories (1-2). Select posts with postType = "customer-story"',
      },
    },
    {
      name: "emptyStateText",
      type: "text",
      label: "Empty State Text",
      admin: {
        description: "Text to display when no entries are found",
      },
    },
    {
      name: "searchPlaceholder",
      type: "text",
      required: true,
      label: "Search Placeholder",
      admin: {
        description: "Placeholder text for the search input",
      },
    },
    {
      name: "defaultFilterTitle",
      type: "text",
      required: true,
      label: "Default Filter Title",
      admin: {
        description: "Title for the default filter option",
      },
    },
    {
      name: "cta",
      type: "relationship",
      relationTo: "ctas",
      label: "Call to Action",
      admin: {
        description: "Optional call to action for the blog index page",
      },
    },
  ],
};

export default BlogIndex;
