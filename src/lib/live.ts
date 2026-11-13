// Live updates: the Host pushes "something changed" topics; screens reload what they show.
import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";

export type Topic = "orders" | "menu" | "cash" | "print" | "settings" | "users" | "devices" | "other" | "all";

/** Call `onChange` when one of `topics` changes (debounced), plus every `fallbackMs` as a safety net. */
export function useLive(topics: Topic[], onChange: () => void, fallbackMs = 15000) {
  const cb = useRef(onChange);
  cb.current = onChange;
  const key = topics.join(",");
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const fire = () => {
      clearTimeout(timer);
      timer = setTimeout(() => cb.current(), 150);
    };
    const un = listen<string>("burger://changed", (e) => {
      if (e.payload === "all" || key.split(",").includes(e.payload)) fire();
    });
    const poll = setInterval(() => cb.current(), fallbackMs);
    return () => {
      clearTimeout(timer);
      clearInterval(poll);
      un.then((f) => f());
    };
  }, [key, fallbackMs]);
}

/** Client devices: is the Host reachable? */
export function useOnline(initial = true) {
  const [online, setOnline] = useState(initial);
  useEffect(() => {
    const un = listen<{ online: boolean }>("burger://connection", (e) => setOnline(e.payload.online));
    return () => void un.then((f) => f());
  }, []);
  return online;
}
