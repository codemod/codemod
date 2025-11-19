import express from "express";
import { getPayload } from "payload";
import config from "./payload.config.js";
import dotenv from "dotenv";

// Load environment variables
dotenv.config();

const app = express();

const PORT = process.env.PORT || 3001;

const start = async () => {
  // Initialize Payload
  const payload = await getPayload({
    config,
  });

  // Add Payload middleware
  // Payload v3 automatically handles /admin and /api routes
  app.use(payload.router);

  app.listen(PORT, () => {
    console.log(`Payload CMS running on http://localhost:${PORT}`);
    console.log(`Admin panel: http://localhost:${PORT}/admin`);
    console.log(`API: http://localhost:${PORT}/api`);
  });
};

start().catch((error) => {
  console.error("Error starting server:", error);
  process.exit(1);
});
