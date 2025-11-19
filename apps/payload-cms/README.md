# Payload CMS Backend

Standalone Payload CMS backend server for content management.

## Development Setup

### Prerequisites

- Node.js 18+
- PostgreSQL 16+ (or Docker)
- pnpm

### Quick Start

1. **Install dependencies:**
   ```bash
   pnpm install
   ```

2. **Set up environment variables:**
   
   Create a `.env` file in `apps/payload-cms/`:
   ```bash
   PAYLOAD_SECRET=your-secret-key-here
   DATABASE_URI=postgresql://user:password@localhost:5432/payload_cms
   PAYLOAD_PUBLIC_SERVER_URL=http://localhost:3001
   PORT=3001
   ```

   Generate a secure secret:
   ```bash
   openssl rand -base64 32
   ```

3. **Set up PostgreSQL database:**

   **Option A: Using Docker (Recommended)**
   ```bash
   docker run --name payload-postgres \
     -e POSTGRES_USER=payload \
     -e POSTGRES_PASSWORD=payload \
     -e POSTGRES_DB=payload_cms \
     -p 5432:5432 \
     -d postgres:16
   ```

   **Option B: Using Homebrew**
   ```bash
   brew install postgresql@16
   brew services start postgresql@16
   createdb payload_cms
   ```

4. **Generate TypeScript types:**
   ```bash
   pnpm generate:types
   ```

5. **Start the development server:**
   ```bash
   pnpm dev
   ```

   The admin panel will be available at `http://localhost:3001/admin`

### First Time Setup

1. Navigate to `http://localhost:3001/admin`
2. Create your first admin user
3. Start creating content

## Available Scripts

- `pnpm dev` - Start development server
- `pnpm build` - Build for production
- `pnpm generate:types` - Generate TypeScript types
- `pnpm generate:importmap` - Generate import map for admin

## API Endpoints

- **Admin Panel**: `http://localhost:3001/admin`
- **REST API**: `http://localhost:3001/api`
- **GraphQL API**: `http://localhost:3001/api/graphql`

## Environment Variables

| Variable | Description | Required | Default |
|----------|-------------|----------|---------|
| `PAYLOAD_SECRET` | Secret key for encryption | Yes | - |
| `DATABASE_URI` | PostgreSQL connection string | Yes | - |
| `PORT` | Server port | No | 3001 |
| `PAYLOAD_PUBLIC_SERVER_URL` | Public server URL | No | http://localhost:3001 |

## Collections

This backend includes the following collections:

- **Blog**: `blog-articles`, `blog-customer-stories`, `blog-authors`, `blog-tags`, `blog-index`
- **Pages**: `pages`, `text-pages`, `pricing`, `contact`, `about`, `careers`, `jobs`, `not-found`
- **Globals**: `navigation`, `footer`, `settings`, `global-labels`, `filter-icon-dictionary`
- **Supporting**: `page-ctas`, `page-cta-doubles`, `page-cta-triples`, `article-ctas`, `tech-features`, `registry-index`
- **Media**: `media`, `users`

## Notes

- Uses Payload's Lexical editor for rich text
- Draft mode is enabled for all collections
- PostgreSQL database adapter is configured
- Custom rich text blocks and section blocks are available
