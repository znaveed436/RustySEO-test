// @ts-nocheck
"use client";

import React, { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Activity,
  Play,
  CheckCircle2,
  XCircle,
  AlertCircle,
  RefreshCw,
  Terminal,
  Sparkles,
  Search,
  BarChart,
  Eye,
  Zap
} from "lucide-react";
import { Button, Loader, Text } from "@mantine/core";

interface DiagnosticResult {
  service: string;
  status: boolean;
  message: string;
  details: string | null;
}

const serviceMeta = {
  ollama: {
    name: "Ollama (Local AI)",
    description: "Local large language model server for content crawling and technical SEO analysis.",
    icon: Terminal,
    color: "text-purple-500 bg-purple-500/10 border-purple-500/20",
  },
  gemini: {
    name: "Google Gemini",
    description: "Cloud-based AI models used for generating descriptions, topics, headings, and schema.",
    icon: Sparkles,
    color: "text-blue-500 bg-blue-500/10 border-blue-500/20",
  },
  gsc: {
    name: "Google Search Console",
    description: "Search query performance, indexation checking, and tracked keywords matcher.",
    icon: Search,
    color: "text-orange-500 bg-orange-500/10 border-orange-500/20",
  },
  ga4: {
    name: "Google Analytics 4",
    description: "Website traffic performance, user engagement, and organic traffic regression stats.",
    icon: BarChart,
    color: "text-yellow-500 bg-yellow-500/10 border-yellow-500/20",
  },
  clarity: {
    name: "Microsoft Clarity",
    description: "Session recordings and user behavior analytics integration.",
    icon: Eye,
    color: "text-teal-500 bg-teal-500/10 border-teal-500/20",
  },
  pagespeed: {
    name: "PageSpeed Insights",
    description: "Lighthouse core web vitals and mobile/desktop performance metrics testing.",
    icon: Zap,
    color: "text-indigo-500 bg-indigo-500/10 border-indigo-500/20",
  },
};

const DiagnosticsSection = () => {
  const [results, setResults] = useState<Record<string, DiagnosticResult>>({});
  const [testing, setTesting] = useState<Record<string, boolean>>({});
  const [globalLoading, setGlobalLoading] = useState(false);

  // Auto-run diagnostics on page load
  useEffect(() => {
    runAllDiagnostics();
  }, []);

  const runAllDiagnostics = async () => {
    try {
      setGlobalLoading(true);
      // Initialize testing status for all services
      const initialTesting = {};
      Object.keys(serviceMeta).forEach((service) => {
        initialTesting[service] = true;
      });
      setTesting(initialTesting);

      const response = await invoke<DiagnosticResult[]>("run_all_integration_diagnostics");

      const newResults = {};
      response.forEach((res) => {
        newResults[res.service] = res;
      });
      setResults(newResults);
    } catch (error) {
      console.error("Failed to run all diagnostics:", error);
    } finally {
      setTesting({});
      setGlobalLoading(false);
    }
  };

  const runSingleDiagnostic = async (service: string) => {
    try {
      setTesting((prev) => ({ ...prev, [service]: true }));
      const res = await invoke<DiagnosticResult>("run_single_integration_diagnostic", { service });
      setResults((prev) => ({ ...prev, [service]: res }));
    } catch (error) {
      console.error(`Failed to run diagnostic for ${service}:`, error);
      setResults((prev) => ({
        ...prev,
        [service]: {
          service,
          status: false,
          message: "Internal Tauri diagnostic command failed",
          details: error?.toString() || "Unknown error",
        }
      }));
    } finally {
      setTesting((prev) => ({ ...prev, [service]: false }));
    }
  };

  return (
    <div className="space-y-4">
      {/* Diagnostics Header & Actions */}
      <div className="flex items-center justify-between p-3 rounded-xl border border-gray-100 dark:border-white/5 bg-gray-50/30 dark:bg-white/[0.01] mb-2">
        <div className="flex items-center gap-2.5">
          <div className="p-2 bg-indigo-500/10 rounded-lg">
            <Activity className="w-4 h-4 text-indigo-500 animate-pulse" />
          </div>
          <div>
            <h4 className="text-[13px] font-bold text-gray-800 dark:text-gray-200">
              Integrations Diagnostics
            </h4>
            <p className="text-[10px] text-gray-400 dark:text-gray-500 mt-0.5">
              Verify local and cloud integrations configured in your desktop environment.
            </p>
          </div>
        </div>
        <Button
          onClick={runAllDiagnostics}
          loading={globalLoading}
          size="xs"
          variant="light"
          color="indigo"
          leftSection={<RefreshCw size={12} className={globalLoading ? "animate-spin" : ""} />}
          className="text-[11px] font-semibold"
        >
          Run All Diagnostics
        </Button>
      </div>

      {/* Grid of Diagnostics */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
        {Object.entries(serviceMeta).map(([key, meta]) => {
          const result = results[key];
          const isTesting = testing[key];
          const IconComponent = meta.icon;

          return (
            <div
              key={key}
              className="flex flex-col p-3.5 rounded-xl border border-gray-150 dark:border-white/5 bg-white dark:bg-brand-dark hover:border-gray-200 dark:hover:border-white/10 transition-all duration-150 shadow-sm"
            >
              {/* Header inside Card */}
              <div className="flex items-start justify-between">
                <div className="flex gap-2.5">
                  <div className={`p-1.5 rounded-lg border shrink-0 ${meta.color}`}>
                    <IconComponent className="w-4 h-4" />
                  </div>
                  <div>
                    <h5 className="text-[12px] font-bold text-gray-800 dark:text-gray-100">
                      {meta.name}
                    </h5>
                    <p className="text-[10.5px] text-gray-400 dark:text-gray-500 leading-snug mt-0.5">
                      {meta.description}
                    </p>
                  </div>
                </div>

                {/* Single Test trigger */}
                <button
                  type="button"
                  onClick={() => runSingleDiagnostic(key)}
                  disabled={isTesting || globalLoading}
                  className="p-1 rounded bg-gray-100 dark:bg-white/5 text-gray-500 hover:text-indigo-500 dark:text-gray-400 hover:bg-indigo-500/10 disabled:opacity-40 transition-colors shrink-0"
                  title="Run specific test"
                >
                  {isTesting ? (
                    <Loader size={12} color="var(--brand-bright)" />
                  ) : (
                    <Play className="w-3 h-3" />
                  )}
                </button>
              </div>

              {/* Status and Details Area */}
              <div className="mt-3.5 pt-3 border-t border-gray-100 dark:border-white/5 flex flex-col gap-1.5 flex-1 justify-end">
                <div className="flex items-center gap-1.5">
                  {isTesting ? (
                    <>
                      <Loader size={10} color="var(--brand-bright)" />
                      <span className="text-[10.5px] text-gray-400 dark:text-gray-500 font-medium">
                        Testing connection...
                      </span>
                    </>
                  ) : result ? (
                    <>
                      {result.status ? (
                        <CheckCircle2 className="w-4 h-4 text-emerald-500 shrink-0" />
                      ) : (
                        <XCircle className="w-4 h-4 text-red-500 shrink-0" />
                      )}
                      <span className={`text-[11px] font-semibold ${result.status ? "text-emerald-600 dark:text-emerald-400" : "text-red-500"}`}>
                        {result.message}
                      </span>
                    </>
                  ) : (
                    <>
                      <AlertCircle className="w-3.5 h-3.5 text-gray-400 shrink-0" />
                      <span className="text-[10.5px] text-gray-400 dark:text-gray-500 font-medium">
                        Not tested yet
                      </span>
                    </>
                  )}
                </div>

                {/* Details collapsible-style text if present */}
                {!isTesting && result && result.details && (
                  <pre className="mt-1 p-2 text-[9.5px] font-mono leading-relaxed bg-gray-50 dark:bg-white/[0.02] border border-gray-100 dark:border-white/5 rounded-lg text-gray-600 dark:text-gray-400 overflow-x-auto whitespace-pre-wrap max-h-24">
                    {result.details}
                  </pre>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
};

export default DiagnosticsSection;
