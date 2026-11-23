import { useEffect, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Api, Project, RunFile } from "../api";

interface Props {
  api: Api;
  project: Project;
  run: string;
  file: RunFile;
}

type Loaded = { text: string } | { error: string } | null;

/** The file rendered as Markdown, raw HTML in it dropped; fetched again whenever its mtime changes. A newer version
 * keeps the older text on show until it arrives; another file shows as loading until it arrives. */
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
  }, [api, project, run, file.name, file.mtime, wanted]);
  const content = loaded.key === wanted ? loaded.content : null;
  if (content === null) {
    return <p className="loading">Loading {file.name}…</p>;
  }
  if ("error" in content) {
    return <p className="problem">{content.error}</p>;
  }
  return (
    <article className="markdown">
      <Markdown remarkPlugins={[remarkGfm]} skipHtml>
        {content.text}
      </Markdown>
    </article>
  );
}
