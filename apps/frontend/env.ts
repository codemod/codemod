import { createEnv } from "@t3-oss/env-nextjs";
import { z } from "zod";

export const env = createEnv({
  /**
   * Run `build` or `dev` with `SKIP_ENV_VALIDATION` to skip env validation. This is especially
   * useful for Docker builds.
   */
  skipValidation: !!process.env.SKIP_ENV_VALIDATION,
  emptyStringAsUndefined: true,
  server: {
    NODE_ENV: z
      .enum(["development", "test", "production"])
      .default("development"),
    DUBCO_WORKSPACE_ID: z.string().optional(),
    DUBCO_API_TOKEN: z.string().optional(),
    SANITY_API_TOKEN: z.string(),
    SANITY_REVALIDATE_SECRET: z.string().optional(),
    HUBSPOT_PORTAL_ID: z.string(),
    HUBSPOT_JOB_FORM_ID: z.string(),
    HUBSPOT_CONTACT_FORM_ID: z.string(),
    HUBSPOT_NEWSLETTER_FORM_ID: z.string(),
    RECAPTCHA_SECRET_KEY: z.string(),
    IS_PREVIEW: z.string().optional(),
    PAYLOAD_SECRET: z.string().optional(),
    DATABASE_URI: z.string().optional(),
    DATABASE_URL: z.string().optional(), // Neon/Vercel standard
  },
  client: {
    NEXT_PUBLIC_API_URL: z.string(),
    NEXT_PUBLIC_AUTH_API_URL: z.string(),
    NEXT_PUBLIC_AI_API_URL: z.string(),
    NEXT_PUBLIC_WS_URL: z.string(),
    NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: z.string(),
    NEXT_PUBLIC_RECAPTCHA_SITE_KEY: z.string(),
    NEXT_PUBLIC_SANITY_PROJECT_ID: z.string(),
    NEXT_PUBLIC_SANITY_DATASET: z.string(),
    NEXT_PUBLIC_SANITY_API_VERSION: z.string().optional(),
    NEXT_PUBLIC_SITE_DOMAIN: z.string().optional(),
    NEXT_PUBLIC_BASE_URL: z.string().optional(),
    NEXT_PUBLIC_CODEMOD_AUTOMATIONS_LIST_ENDPOINT: z.string(),
    NEXT_PUBLIC_CODEMOD_AUTOMATIONS_LIST_ENDPOINT_OLD: z.string(),
  },

  /**
   * You can't destruct `process.env` as a regular object in the Next.js edge runtimes (e.g.
   * middlewares) or client-side so we need to destruct manually.
   */
  runtimeEnv: {
    NODE_ENV: process.env.NODE_ENV,
    DUBCO_WORKSPACE_ID: process.env.DUBCO_WORKSPACE_ID,
    DUBCO_API_TOKEN: process.env.DUBCO_API_TOKEN,
    NEXT_PUBLIC_API_URL: process.env.NEXT_PUBLIC_API_URL,
    NEXT_PUBLIC_AUTH_API_URL: process.env.NEXT_PUBLIC_AUTH_API_URL,
    NEXT_PUBLIC_AI_API_URL: process.env.NEXT_PUBLIC_AI_API_URL,
    NEXT_PUBLIC_WS_URL: process.env.NEXT_PUBLIC_WS_URL,
    NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY:
      process.env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY,
    NEXT_PUBLIC_RECAPTCHA_SITE_KEY: process.env.NEXT_PUBLIC_RECAPTCHA_SITE_KEY,
    SANITY_API_TOKEN: process.env.SANITY_API_TOKEN,
    SANITY_REVALIDATE_SECRET: process.env.SANITY_REVALIDATE_SECRET,
    NEXT_PUBLIC_SANITY_PROJECT_ID: process.env.NEXT_PUBLIC_SANITY_PROJECT_ID,
    NEXT_PUBLIC_SANITY_DATASET: process.env.NEXT_PUBLIC_SANITY_DATASET,
    NEXT_PUBLIC_SANITY_API_VERSION: process.env.NEXT_PUBLIC_SANITY_API_VERSION,
    NEXT_PUBLIC_SITE_DOMAIN: process.env.NEXT_PUBLIC_SITE_DOMAIN,
    NEXT_PUBLIC_BASE_URL: process.env.NEXT_PUBLIC_BASE_URL,
    NEXT_PUBLIC_CODEMOD_AUTOMATIONS_LIST_ENDPOINT:
      process.env.NEXT_PUBLIC_CODEMOD_AUTOMATIONS_LIST_ENDPOINT,
    NEXT_PUBLIC_CODEMOD_AUTOMATIONS_LIST_ENDPOINT_OLD:
      process.env.NEXT_PUBLIC_CODEMOD_AUTOMATIONS_LIST_ENDPOINT_OLD,
    HUBSPOT_PORTAL_ID: process.env.HUBSPOT_PORTAL_ID,
    HUBSPOT_JOB_FORM_ID: process.env.HUBSPOT_JOB_FORM_ID,
    HUBSPOT_CONTACT_FORM_ID: process.env.HUBSPOT_CONTACT_FORM_ID,
    HUBSPOT_NEWSLETTER_FORM_ID: process.env.HUBSPOT_NEWSLETTER_FORM_ID,
    RECAPTCHA_SECRET_KEY: process.env.RECAPTCHA_SECRET_KEY,
    IS_PREVIEW: process.env.IS_PREVIEW,
    PAYLOAD_SECRET: process.env.PAYLOAD_SECRET,
    DATABASE_URI: process.env.DATABASE_URI,
    DATABASE_URL: process.env.DATABASE_URL, // Neon/Vercel standard
  },
});
