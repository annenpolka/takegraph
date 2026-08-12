import { createMcpExpressApp } from "@modelcontextprotocol/sdk/server/express.js";
import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import cors from "cors";
import type { Request, Response } from "express";
import { ProjectSession } from "./project-session.js";
import { createServer } from "./server.js";

async function startStdioServer(factory: () => McpServer): Promise<void> {
  await factory().connect(new StdioServerTransport());
}

async function startHttpServer(factory: () => McpServer): Promise<void> {
  const port = Number.parseInt(process.env.PORT ?? "3001", 10);
  const app = createMcpExpressApp({ host: "127.0.0.1" });
  app.use(cors());

  app.all("/mcp", async (request: Request, response: Response) => {
    const server = factory();
    const transport = new StreamableHTTPServerTransport({
      sessionIdGenerator: undefined,
    });

    response.on("close", () => {
      void transport.close();
      void server.close();
    });

    try {
      await server.connect(transport);
      await transport.handleRequest(request, response, request.body);
    } catch (error) {
      console.error("MCP request failed", error);
      if (!response.headersSent) {
        response.status(500).json({
          jsonrpc: "2.0",
          error: { code: -32_603, message: "Internal server error" },
          id: null,
        });
      }
    }
  });

  app.listen(port, "127.0.0.1", () => {
    console.error(`TakeGraph MCP listening on http://127.0.0.1:${port}/mcp`);
  });
}

const session = new ProjectSession();
const serverFactory = () => createServer({ session });

if (process.argv.includes("--stdio")) {
  await startStdioServer(serverFactory);
} else {
  await startHttpServer(serverFactory);
}
