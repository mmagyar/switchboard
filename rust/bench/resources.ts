import { z } from "zod";

const ticksSchema = z.coerce.number().int().nonnegative();
export const parseCpuTicks = (stat: string): number => {
  // comm may contain whitespace and parentheses; fields following its final ')' are fixed.
  const end = stat.lastIndexOf(")");
  if (end < 0) throw new Error("Invalid /proc stat: missing comm");
  const fields = stat.slice(end + 1).trim().split(/\s+/);
  const user = ticksSchema.safeParse(fields[11]);
  const system = ticksSchema.safeParse(fields[12]);
  if (!user.success || !system.success) throw new Error("Invalid /proc stat CPU fields");
  return user.data + system.data;
};

export type Resources = {
  readonly cpuSeconds: number;
  readonly rssBytes: number;
  readonly peakRssBytes: number;
};

export const parseMemory = (status: string): Pick<Resources, "rssBytes" | "peakRssBytes"> => {
  const field = (name: string): number => {
    const match = status.match(new RegExp(`^${name}:\\s+(\\d+) kB$`, "m"));
    const result = ticksSchema.safeParse(match?.[1]);
    if (!result.success) throw new Error(`Invalid /proc status ${name}`);
    return result.data * 1024;
  };
  return { rssBytes: field("VmRSS"), peakRssBytes: field("VmHWM") };
};

export const readResources = async (pid: number, clockTicks: number): Promise<Resources> => {
  const [stat, status] = await Promise.all([
    Bun.file(`/proc/${pid}/stat`).text(), Bun.file(`/proc/${pid}/status`).text(),
  ]);
  return { cpuSeconds: parseCpuTicks(stat) / clockTicks, ...parseMemory(status) };
};

const command = async (cmd: readonly string[]): Promise<string> => {
  const child = Bun.spawn([...cmd], { stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const timer = setTimeout(() => child.kill("SIGKILL"), 5000);
  try {
    const [stdout, stderr, exit] = await Promise.all([
      new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
    ]);
    if (exit !== 0) throw new Error(`${cmd.join(" ")} exited ${exit}: ${stderr}`);
    return stdout.trim();
  } finally {
    clearTimeout(timer);
    if (child.exitCode === null) child.kill("SIGKILL");
    await child.exited;
  }
};

const optionalText = async (path: string): Promise<string | null> => {
  const file = Bun.file(path);
  return await file.exists() ? (await file.text()).trim() : null;
};

export const environment = async (): Promise<{
  readonly clockTicks: number;
  readonly details: Readonly<Record<string, unknown>>;
}> => {
  if (process.platform !== "linux") throw new Error("Benchmark resource accounting requires Linux /proc");
  const ticksText = await command(["getconf", "CLK_TCK"]);
  const ticks = z.coerce.number().int().positive().safeParse(ticksText);
  if (!ticks.success) throw new Error(`Invalid CLK_TCK: ${ticksText}`);
  const cgroup = await Bun.file("/proc/self/cgroup").text();
  const unified = cgroup.match(/^0::(.+)$/m)?.[1];
  const cgroupPath = unified === undefined ? null : `/sys/fs/cgroup${unified === "/" ? "" : unified}`;
  const cpuInfo = await Bun.file("/proc/cpuinfo").text();
  const [kernel, rustc, cargo, cpuMax, rootCpuMax, legacyQuota, legacyPeriod, cpuset] = await Promise.all([
    command(["uname", "-a"]), command(["rustc", "--version", "--verbose"]), command(["cargo", "--version"]),
    cgroupPath === null ? optionalText("/sys/fs/cgroup/cpu.max") : optionalText(`${cgroupPath}/cpu.max`),
    optionalText("/sys/fs/cgroup/cpu.max"),
    optionalText("/sys/fs/cgroup/cpu/cpu.cfs_quota_us"), optionalText("/sys/fs/cgroup/cpu/cpu.cfs_period_us"),
    cgroupPath === null ? optionalText("/sys/fs/cgroup/cpuset.cpus.effective") : optionalText(`${cgroupPath}/cpuset.cpus.effective`),
  ]);
  return {
    clockTicks: ticks.data,
    details: {
      kernel, rustc, cargo, bun: Bun.version, platform: process.platform, architecture: process.arch,
      cpuModel: cpuInfo.match(/^model name\s*:\s*(.+)$/m)?.[1] ?? null,
      logicalCpus: cpuInfo.match(/^processor\s*:/gm)?.length ?? 0,
      cpuAllowedList: (await Bun.file("/proc/self/status").text()).match(/^Cpus_allowed_list:\s*(.+)$/m)?.[1] ?? null,
      cgroup: cgroup.trim(), cgroupPath, cpuMax, rootCpuMax, legacyQuota, legacyPeriod, cpuset,
      clockTicks: ticks.data,
    },
  };
};
