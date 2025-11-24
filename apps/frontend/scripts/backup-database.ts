#!/usr/bin/env tsx
/**
 * Complete database backup for Payload CMS
 * Backs up all collections and globals to a timestamped JSON file
 */

import { config } from "dotenv";
import * as path from "path";
import * as fs from "fs";
config({ path: path.resolve(process.cwd(), ".env.local") });

async function backupDatabase() {
  console.log("💾 Starting database backup...\n");

  const { getPayload } = await import("payload");
  const { postgresAdapter } = await import("@payloadcms/db-postgres");
  const configPromise = await import("../payload.config").then(
    (m) => m.default,
  );
  const baseConfig = await configPromise;
  const payload = await getPayload({
    config: {
      ...baseConfig,
      secret: process.env.PAYLOAD_SECRET!,
      db: postgresAdapter({
        pool: {
          connectionString: process.env.DATABASE_URI!,
        },
      }),
    },
  });

  try {
    const backup: any = {
      timestamp: new Date().toISOString(),
      collections: {},
      globals: {},
    };

    // Backup all collections
    console.log("📦 Backing up collections...");
    const collections = (payload.config.collections || []).map(
      (col: any) => col.slug,
    );

    for (const collectionSlug of collections) {
      try {
        const result = await payload.find({
          collection: collectionSlug as any,
          limit: 10000, // Get all documents
          depth: 0, // Don't populate relations to avoid circular refs
        });

        backup.collections[collectionSlug] = {
          totalDocs: result.totalDocs,
          docs: result.docs,
        };
        console.log(`   ✅ ${collectionSlug}: ${result.totalDocs} document(s)`);
      } catch (error: any) {
        console.warn(`   ⚠️  ${collectionSlug}: ${error?.message || error}`);
        backup.collections[collectionSlug] = {
          error: error?.message || String(error),
        };
      }
    }

    // Backup all globals
    console.log("\n🌐 Backing up globals...");
    const globals = (payload.config.globals || []).map(
      (glob: any) => glob.slug,
    );

    for (const globalSlug of globals) {
      try {
        const global = await payload.findGlobal({
          slug: globalSlug as any,
        });

        backup.globals[globalSlug] = global || null;
        console.log(`   ✅ ${globalSlug}: ${global ? "Found" : "Not found"}`);
      } catch (error: any) {
        console.warn(`   ⚠️  ${globalSlug}: ${error?.message || error}`);
        backup.globals[globalSlug] = {
          error: error?.message || String(error),
        };
      }
    }

    // Save backup to file
    const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
    const backupDir = path.resolve(process.cwd(), "scripts", "backups");
    if (!fs.existsSync(backupDir)) {
      fs.mkdirSync(backupDir, { recursive: true });
    }

    const backupPath = path.resolve(
      backupDir,
      `payload-backup-${timestamp}.json`,
    );
    fs.writeFileSync(backupPath, JSON.stringify(backup, null, 2));

    const stats = fs.statSync(backupPath);
    const sizeMB = (stats.size / (1024 * 1024)).toFixed(2);

    console.log(`\n✅ Backup complete!`);
    console.log(`   📁 Location: ${backupPath}`);
    console.log(`   📊 Size: ${sizeMB} MB`);
    console.log(`   📦 Collections: ${Object.keys(backup.collections).length}`);
    console.log(`   🌐 Globals: ${Object.keys(backup.globals).length}`);
    console.log(
      `\n💡 To restore, use: pnpm dlx tsx scripts/restore-database.ts ${backupPath}`,
    );

    await payload.db.destroy?.();
    process.exit(0);
  } catch (error: any) {
    console.error(`\n❌ Backup failed: ${error?.message || error}`);
    await payload.db.destroy?.();
    process.exit(1);
  }
}

backupDatabase();
