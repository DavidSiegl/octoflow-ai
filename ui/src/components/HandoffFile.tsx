import { useEffect, useState, type ComponentProps } from "react";
import Markdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Api, Project, RunFile } from "../api";
import { OutsideLink } from "./OutsideLink";

interface Props {
  api: Api;
  project: Project;
  run: string;
  file: RunFile;
}

type Loaded = { text: string } | { error: string } | null;

/** A Markdown link in a handoff file, never an `<a href>` (OutsideLink says why): an absolute `https` link is an
 * OutsideLink; any other (relative, `file:`, `http:`, …) is its text, marked as a link the app does not follow. Either
 * shows its target as the tooltip, after the link's own title when it has one. */
function HandoffLink({ href, title, children }: ComponentProps<"a">) {
  const tooltip = [title, href].filter(Boolean).join(" — ");
  if (href && isHttps(href)) {
    return (
      <OutsideLink target={{ page: href }} title={tooltip}>
        {children}
      </OutsideLink>
    );
  }
  return (
    <span className="unfollowed-link" title={`${tooltip} (not opened: only an https link opens, outside the app)`}>
      {children}
    </span>
  );
}

/** Whether href is an absolute `https` URL. */
function isHttps(href: string) {
  try {
    return new URL(href).protocol === "https:";
  } catch {
    return false;
  }
}

const COMPONENTS: Components = { a: HandoffLink };

/** The file rendered as Markdown, raw HTML in it dropped and its links never followed in the window (HandoffLink);
 * fetched again whenever its mtime or its size changes: the mtime is in whole seconds, so a write within the second of
 * the one read, which can have caught the first partway, changes only the size. A newer version keeps the older text
 * on show until it arrives; another file shows as loading until it arrives. */
export function HandoffFile({ api, project, run, file }: Props) {
  // What was last fetched, keyed by the file it is of but not the version: a newer version of the file on show
  // replaces the text only once it arrives, so the panel neither flashes nor changes height, while another file
  // selected since shows as loading, never as the last one.
  const [loaded, setLoaded] = useState<{ key: string; content: Loaded }>({ key: "", content: null });
  // The key of the file asked for, any version.
  const wanted = JSON.stringify([project, run, file.name]);
  useEffect(() => {
    let current = true;
    api.fileText(project, run, file.name).then(
      (text) => current && setLoaded({ key: wanted, content: { text } }),
      (error: Error) => current && setLoaded({ key: wanted, content: { error: error.message } }),
    );
    return () => {
      current = false;
    };
  }, [api, project, run, file.name, file.mtime, file.size, wanted]);
  const content = loaded.key === wanted ? loaded.content : null;
  if (content === null) {
    return <p className="loading">Loading {file.name}…</p>;
  }
  if ("error" in content) {
    return <p className="problem">{content.error}</p>;
  }
  return (
    <article className="markdown">
      <Markdown remarkPlugins={[remarkGfm]} components={COMPONENTS} skipHtml>
        {content.text}
      </Markdown>
    </article>
  );
}
