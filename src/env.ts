import { env } from "bun";

export const VerboseErrorOutput = env["VERBOSE_ERROR_OUTPUT"] === "true";
