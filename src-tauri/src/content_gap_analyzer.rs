//! Deterministic content-gap analysis for Google Search Console query data and
//! crawled-page content.
//!
//! The analyser is intentionally transparent: it uses query metrics, lexical
//! coverage, landing-page mapping and intent signals. It does not invent search
//! volume or present impression-based estimates as third-party keyword volume.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use strsim::jaro_winkler;
use url::Url;

const MAX_GSC_ROWS: usize = 250_000;
const MAX_CRAWLED_PAGES: usize = 100_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GscQueryInput {
    pub query: String,
    #[serde(default)]
    pub page: Option<String>,
    #[serde(default)]
    pub clicks: f64,
    #[serde(default)]
    pub impressions: f64,
    #[serde(default)]
    pub ctr: f64,
    #[serde(default)]
    pub position: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CrawledPageInput {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub meta_description: String,
    #[serde(default)]
    pub headings: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContentGapSettings {
    pub minimum_impressions: f64,
    pub striking_distance_min: f64,
    pub striking_distance_max: f64,
    pub low_ctr_ratio: f64,
    pub minimum_page_coverage: f64,
    pub cluster_similarity: f64,
    pub max_secondary_keywords: usize,
    pub max_clusters: usize,
}

impl Default for ContentGapSettings {
    fn default() -> Self {
        Self {
            minimum_impressions: 20.0,
            striking_distance_min: 8.0,
            striking_distance_max: 20.0,
            low_ctr_ratio: 0.60,
            minimum_page_coverage: 0.32,
            cluster_similarity: 0.56,
            max_secondary_keywords: 12,
            max_clusters: 500,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SearchIntent {
    Informational,
    Commercial,
    Transactional,
    Local,
    Navigational,
    Mixed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum GapType {
    NoRelevantLandingPage,
    StrikingDistance,
    HighImpressionLowCtr,
    UnderperformingSubtopic,
    AccidentalRanking,
    Cannibalisation,
    LongTailOpportunity,
    QuestionOpportunity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryOpportunity {
    pub query: String,
    pub clicks: f64,
    pub impressions: f64,
    pub ctr: f64,
    pub position: f64,
    pub expected_ctr: f64,
    pub estimated_click_uplift: f64,
    pub intent: SearchIntent,
    pub gap_types: Vec<GapType>,
    pub mapped_page: Option<String>,
    pub mapped_page_coverage: f64,
    pub ranking_pages: Vec<String>,
    pub opportunity_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentGapCluster {
    pub cluster_id: String,
    pub primary_keyword: String,
    pub secondary_keywords: Vec<String>,
    pub intent: SearchIntent,
    pub gap_types: Vec<GapType>,
    pub total_clicks: f64,
    pub total_impressions: f64,
    pub weighted_position: f64,
    pub weighted_ctr: f64,
    pub estimated_click_uplift: f64,
    pub opportunity_score: f64,
    pub mapped_page: Option<String>,
    pub average_page_coverage: f64,
    pub recommended_action: String,
    pub suggested_slug: String,
    pub suggested_title: String,
    pub suggested_h2s: Vec<String>,
    pub queries: Vec<QueryOpportunity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContentGapSummary {
    pub input_query_rows: usize,
    pub unique_queries: usize,
    pub eligible_queries: usize,
    pub crawled_pages: usize,
    pub clusters: usize,
    pub no_relevant_page_queries: usize,
    pub striking_distance_queries: usize,
    pub low_ctr_queries: usize,
    pub cannibalised_queries: usize,
    pub estimated_click_uplift: f64,
    pub covered_query_percentage: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentGapReport {
    pub domain: String,
    pub summary: ContentGapSummary,
    pub settings: ContentGapSettings,
    pub clusters: Vec<ContentGapCluster>,
    pub excluded_rows: usize,
    pub methodology_notes: Vec<String>,
}

#[derive(Debug, Clone)]
struct PageIndex {
    url: String,
    title_normalised: String,
    headings_normalised: String,
    keywords_normalised: String,
    all_tokens: HashSet<String>,
}

#[derive(Debug, Clone, Default)]
struct AggregatedQuery {
    query: String,
    clicks: f64,
    impressions: f64,
    weighted_position_sum: f64,
    ranking_pages: HashMap<String, f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct GapCsvRow<'a> {
    cluster_id: &'a str,
    primary_keyword: &'a str,
    secondary_keywords: String,
    intent: &'a str,
    gap_types: String,
    total_clicks: f64,
    total_impressions: f64,
    weighted_position: f64,
    weighted_ctr: f64,
    estimated_click_uplift: f64,
    opportunity_score: f64,
    mapped_page: &'a str,
    average_page_coverage: f64,
    recommended_action: &'a str,
    suggested_slug: &'a str,
    suggested_title: &'a str,
    suggested_h2s: String,
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn analyze_content_gaps_command(
    domain: String,
    gsc_queries: Vec<GscQueryInput>,
    crawled_pages: Option<Vec<CrawledPageInput>>,
    crawled_urls: Option<Vec<String>>,
    crawled_keywords: Option<Vec<String>>,
    settings: Option<ContentGapSettings>,
) -> Result<ContentGapReport, String> {
    if domain.trim().is_empty() {
        return Err("domain cannot be empty".to_string());
    }
    if gsc_queries.is_empty() {
        return Err("gscQueries cannot be empty".to_string());
    }
    if gsc_queries.len() > MAX_GSC_ROWS {
        return Err(format!(
            "gscQueries exceeds the maximum of {MAX_GSC_ROWS} rows"
        ));
    }

    let pages = merge_page_inputs(crawled_pages, crawled_urls, crawled_keywords);
    if pages.len() > MAX_CRAWLED_PAGES {
        return Err(format!(
            "crawledPages exceeds the maximum of {MAX_CRAWLED_PAGES} pages"
        ));
    }

    let resolved_settings = sanitise_settings(settings.unwrap_or_default());
    tokio::task::spawn_blocking(move || {
        analyse_content_gaps(domain, gsc_queries, pages, resolved_settings)
    })
    .await
    .map_err(|error| format!("Content-gap analysis task failed: {error}"))?
}

#[tauri::command]
pub fn export_gap_analysis_csv_command(analysis: ContentGapReport) -> Result<String, String> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(true)
        .from_writer(Vec::<u8>::new());

    for cluster in &analysis.clusters {
        writer
            .serialize(GapCsvRow {
                cluster_id: &cluster.cluster_id,
                primary_keyword: &cluster.primary_keyword,
                secondary_keywords: cluster.secondary_keywords.join(" | "),
                intent: intent_label(cluster.intent),
                gap_types: cluster
                    .gap_types
                    .iter()
                    .map(|gap| gap_type_label(*gap))
                    .collect::<Vec<_>>()
                    .join(" | "),
                total_clicks: round(cluster.total_clicks, 2),
                total_impressions: round(cluster.total_impressions, 2),
                weighted_position: round(cluster.weighted_position, 2),
                weighted_ctr: round(cluster.weighted_ctr, 4),
                estimated_click_uplift: round(cluster.estimated_click_uplift, 2),
                opportunity_score: round(cluster.opportunity_score, 2),
                mapped_page: cluster.mapped_page.as_deref().unwrap_or(""),
                average_page_coverage: round(cluster.average_page_coverage, 4),
                recommended_action: &cluster.recommended_action,
                suggested_slug: &cluster.suggested_slug,
                suggested_title: &cluster.suggested_title,
                suggested_h2s: cluster.suggested_h2s.join(" | "),
            })
            .map_err(|error| format!("Failed to serialise content-gap CSV: {error}"))?;
    }

    writer
        .flush()
        .map_err(|error| format!("Failed to finish content-gap CSV: {error}"))?;
    let bytes = writer
        .into_inner()
        .map_err(|error| format!("Failed to build content-gap CSV: {}", error.into_error()))?;

    String::from_utf8(bytes).map_err(|error| format!("CSV was not valid UTF-8: {error}"))
}

fn analyse_content_gaps(
    domain: String,
    gsc_queries: Vec<GscQueryInput>,
    crawled_pages: Vec<CrawledPageInput>,
    settings: ContentGapSettings,
) -> Result<ContentGapReport, String> {
    let normalised_domain = normalise_domain(&domain)?;
    let page_index = build_page_index(crawled_pages);
    let input_query_rows = gsc_queries.len();
    let (aggregated, excluded_rows) = aggregate_queries(gsc_queries);
    let unique_queries = aggregated.len();

    let mut opportunities = Vec::new();
    for query in aggregated.into_values() {
        if query.impressions < settings.minimum_impressions {
            continue;
        }
        opportunities.push(score_query_opportunity(&query, &page_index, &settings));
    }

    opportunities.sort_by(compare_query_opportunities);
    let eligible_queries = opportunities.len();
    let mut clusters = cluster_opportunities(opportunities, &settings);
    clusters.sort_by(|left, right| {
        right
            .opportunity_score
            .partial_cmp(&left.opportunity_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                right
                    .total_impressions
                    .partial_cmp(&left.total_impressions)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| left.primary_keyword.cmp(&right.primary_keyword))
    });
    clusters.truncate(settings.max_clusters);

    let all_queries = clusters
        .iter()
        .flat_map(|cluster| cluster.queries.iter())
        .collect::<Vec<_>>();
    let no_relevant_page_queries = all_queries
        .iter()
        .filter(|query| query.gap_types.contains(&GapType::NoRelevantLandingPage))
        .count();
    let striking_distance_queries = all_queries
        .iter()
        .filter(|query| query.gap_types.contains(&GapType::StrikingDistance))
        .count();
    let low_ctr_queries = all_queries
        .iter()
        .filter(|query| query.gap_types.contains(&GapType::HighImpressionLowCtr))
        .count();
    let cannibalised_queries = all_queries
        .iter()
        .filter(|query| query.gap_types.contains(&GapType::Cannibalisation))
        .count();
    let estimated_click_uplift = all_queries
        .iter()
        .map(|query| query.estimated_click_uplift)
        .sum::<f64>();
    let covered_queries = all_queries
        .iter()
        .filter(|query| query.mapped_page_coverage >= settings.minimum_page_coverage)
        .count();
    let covered_query_percentage = if all_queries.is_empty() {
        0.0
    } else {
        (covered_queries as f64 / all_queries.len() as f64) * 100.0
    };

    Ok(ContentGapReport {
        domain: normalised_domain,
        summary: ContentGapSummary {
            input_query_rows,
            unique_queries,
            eligible_queries,
            crawled_pages: page_index.len(),
            clusters: clusters.len(),
            no_relevant_page_queries,
            striking_distance_queries,
            low_ctr_queries,
            cannibalised_queries,
            estimated_click_uplift: round(estimated_click_uplift, 2),
            covered_query_percentage: round(covered_query_percentage, 2),
        },
        settings,
        clusters,
        excluded_rows,
        methodology_notes: vec![
            "Demand is based on supplied Google Search Console impressions, not third-party search volume."
                .to_string(),
            "Estimated click uplift compares observed CTR with a conservative position-based CTR curve and is directional rather than a forecast guarantee."
                .to_string(),
            "Page coverage is a lexical relevance score across titles, headings, declared keywords and body content."
                .to_string(),
            "Clusters are deterministic lexical groups and should be reviewed before content production."
                .to_string(),
        ],
    })
}

fn aggregate_queries(rows: Vec<GscQueryInput>) -> (HashMap<String, AggregatedQuery>, usize) {
    let mut aggregated: HashMap<String, AggregatedQuery> = HashMap::new();
    let mut excluded = 0;

    for row in rows {
        let query = normalise_text(&row.query);
        if query.is_empty()
            || !row.clicks.is_finite()
            || !row.impressions.is_finite()
            || !row.ctr.is_finite()
            || !row.position.is_finite()
            || row.clicks < 0.0
            || row.impressions < 0.0
            || row.position < 0.0
        {
            excluded += 1;
            continue;
        }

        let impressions = row.impressions.max(row.clicks);
        let position_weight = impressions.max(1.0);
        let entry = aggregated
            .entry(query.clone())
            .or_insert_with(|| AggregatedQuery {
                query,
                ..AggregatedQuery::default()
            });
        entry.clicks += row.clicks;
        entry.impressions += impressions;
        entry.weighted_position_sum += row.position * position_weight;

        if let Some(page) = row
            .page
            .as_deref()
            .map(normalise_url)
            .filter(|page| !page.is_empty())
        {
            *entry.ranking_pages.entry(page).or_default() += impressions;
        }
    }

    (aggregated, excluded)
}

fn score_query_opportunity(
    query: &AggregatedQuery,
    pages: &[PageIndex],
    settings: &ContentGapSettings,
) -> QueryOpportunity {
    let ctr = safe_ratio(query.clicks, query.impressions);
    let position = if query.impressions > 0.0 {
        query.weighted_position_sum / query.impressions.max(1.0)
    } else {
        0.0
    };
    let expected_ctr = expected_ctr_for_position(position);
    let estimated_click_uplift = ((expected_ctr - ctr).max(0.0) * query.impressions).max(0.0);
    let query_tokens = meaningful_tokens(&query.query);
    let ranking_pages = sorted_ranking_pages(&query.ranking_pages);

    let explicit_page = ranking_pages
        .first()
        .and_then(|url| pages.iter().find(|page| page.url == url.as_str()));
    let best_page = explicit_page
        .map(|page| (page, page_coverage(&query.query, &query_tokens, page)))
        .or_else(|| best_matching_page(&query.query, &query_tokens, pages));
    let mapped_page = best_page.map(|(page, _)| page.url.clone());
    let coverage = best_page.map(|(_, score)| score).unwrap_or(0.0);

    let mut gap_types = BTreeSet::new();
    if mapped_page.is_none() || coverage < settings.minimum_page_coverage {
        gap_types.insert(GapType::NoRelevantLandingPage);
    }
    if position >= settings.striking_distance_min && position <= settings.striking_distance_max {
        gap_types.insert(GapType::StrikingDistance);
    }
    if query.impressions >= settings.minimum_impressions
        && ctr < expected_ctr * settings.low_ctr_ratio
    {
        gap_types.insert(GapType::HighImpressionLowCtr);
    }
    if !ranking_pages.is_empty() && coverage < 0.20 {
        gap_types.insert(GapType::AccidentalRanking);
    } else if coverage >= 0.20 && coverage < 0.58 && position <= 30.0 {
        gap_types.insert(GapType::UnderperformingSubtopic);
    }
    if ranking_pages.len() > 1 {
        gap_types.insert(GapType::Cannibalisation);
    }
    if query_tokens.len() >= 4 {
        gap_types.insert(GapType::LongTailOpportunity);
    }
    if is_question_query(&query.query) {
        gap_types.insert(GapType::QuestionOpportunity);
    }

    let intent = classify_intent(&query.query);
    let score = opportunity_score(
        query.impressions,
        position,
        ctr,
        expected_ctr,
        coverage,
        intent,
        &gap_types,
    );

    QueryOpportunity {
        query: query.query.clone(),
        clicks: round(query.clicks, 2),
        impressions: round(query.impressions, 2),
        ctr: round(ctr, 4),
        position: round(position, 2),
        expected_ctr: round(expected_ctr, 4),
        estimated_click_uplift: round(estimated_click_uplift, 2),
        intent,
        gap_types: gap_types.into_iter().collect(),
        mapped_page,
        mapped_page_coverage: round(coverage, 4),
        ranking_pages,
        opportunity_score: round(score, 2),
    }
}

fn cluster_opportunities(
    opportunities: Vec<QueryOpportunity>,
    settings: &ContentGapSettings,
) -> Vec<ContentGapCluster> {
    let mut groups: Vec<Vec<QueryOpportunity>> = Vec::new();

    for opportunity in opportunities {
        let mut best_group: Option<(usize, f64)> = None;
        for (index, group) in groups.iter().enumerate() {
            let representative = &group[0].query;
            let score = query_similarity(&opportunity.query, representative);
            if score >= settings.cluster_similarity
                && best_group
                    .as_ref()
                    .map(|(_, existing)| score > *existing)
                    .unwrap_or(true)
            {
                best_group = Some((index, score));
            }
        }

        if let Some((index, _)) = best_group {
            groups[index].push(opportunity);
        } else {
            groups.push(vec![opportunity]);
        }
    }

    groups
        .into_iter()
        .enumerate()
        .map(|(index, group)| build_cluster(index + 1, group, settings))
        .collect()
}

fn build_cluster(
    cluster_number: usize,
    mut queries: Vec<QueryOpportunity>,
    settings: &ContentGapSettings,
) -> ContentGapCluster {
    queries.sort_by(compare_query_opportunities);
    let primary = queries
        .first()
        .map(|query| query.query.clone())
        .unwrap_or_default();
    let total_impressions = queries.iter().map(|query| query.impressions).sum::<f64>();
    let total_clicks = queries.iter().map(|query| query.clicks).sum::<f64>();
    let weighted_position = weighted_average(
        queries
            .iter()
            .map(|query| (query.position, query.impressions.max(1.0))),
    );
    let weighted_ctr = safe_ratio(total_clicks, total_impressions);
    let estimated_click_uplift = queries
        .iter()
        .map(|query| query.estimated_click_uplift)
        .sum::<f64>();
    let average_page_coverage = weighted_average(
        queries
            .iter()
            .map(|query| (query.mapped_page_coverage, query.impressions.max(1.0))),
    );
    let opportunity_score = weighted_average(
        queries
            .iter()
            .map(|query| (query.opportunity_score, query.impressions.max(1.0))),
    );

    let intent = dominant_intent(&queries);
    let gap_types = queries
        .iter()
        .flat_map(|query| query.gap_types.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mapped_page = dominant_page(&queries);
    let secondary_keywords = queries
        .iter()
        .skip(1)
        .map(|query| query.query.clone())
        .take(settings.max_secondary_keywords)
        .collect::<Vec<_>>();
    let recommended_action = recommend_action(&gap_types, mapped_page.as_deref(), &primary);
    let suggested_slug = create_slug(&primary);
    let suggested_title = create_title(&primary, intent);
    let suggested_h2s = create_h2s(&primary, &secondary_keywords, intent);

    ContentGapCluster {
        cluster_id: format!("gap-{cluster_number:04}"),
        primary_keyword: primary,
        secondary_keywords,
        intent,
        gap_types,
        total_clicks: round(total_clicks, 2),
        total_impressions: round(total_impressions, 2),
        weighted_position: round(weighted_position, 2),
        weighted_ctr: round(weighted_ctr, 4),
        estimated_click_uplift: round(estimated_click_uplift, 2),
        opportunity_score: round(opportunity_score, 2),
        mapped_page,
        average_page_coverage: round(average_page_coverage, 4),
        recommended_action,
        suggested_slug,
        suggested_title,
        suggested_h2s,
        queries,
    }
}

fn build_page_index(pages: Vec<CrawledPageInput>) -> Vec<PageIndex> {
    pages
        .into_iter()
        .filter_map(|page| {
            let url = normalise_url(&page.url);
            if url.is_empty() {
                return None;
            }

            let title_normalised = normalise_text(&page.title);
            let headings_normalised = normalise_text(&page.headings.join(" "));
            let keywords_normalised = normalise_text(&page.keywords.join(" "));
            let content_normalised = normalise_text(&format!(
                "{} {}",
                page.meta_description, page.content
            ));
            let all_tokens = meaningful_tokens(&format!(
                "{} {} {} {}",
                title_normalised,
                headings_normalised,
                keywords_normalised,
                content_normalised
            ));

            Some(PageIndex {
                url,
                title_normalised,
                headings_normalised,
                keywords_normalised,
                all_tokens,
            })
        })
        .collect()
}

fn merge_page_inputs(
    crawled_pages: Option<Vec<CrawledPageInput>>,
    crawled_urls: Option<Vec<String>>,
    crawled_keywords: Option<Vec<String>>,
) -> Vec<CrawledPageInput> {
    if let Some(pages) = crawled_pages.filter(|pages| !pages.is_empty()) {
        return pages;
    }

    let urls = crawled_urls.unwrap_or_default();
    let keywords = crawled_keywords.unwrap_or_default();
    urls.into_iter()
        .enumerate()
        .map(|(index, url)| CrawledPageInput {
            url,
            keywords: keywords.get(index).cloned().into_iter().collect(),
            ..CrawledPageInput::default()
        })
        .collect()
}

fn best_matching_page<'a>(
    query: &str,
    query_tokens: &HashSet<String>,
    pages: &'a [PageIndex],
) -> Option<(&'a PageIndex, f64)> {
    pages
        .iter()
        .map(|page| (page, page_coverage(query, query_tokens, page)))
        .max_by(|left, right| left.1.partial_cmp(&right.1).unwrap_or(Ordering::Equal))
}

fn page_coverage(query: &str, query_tokens: &HashSet<String>, page: &PageIndex) -> f64 {
    if query_tokens.is_empty() {
        return 0.0;
    }

    let title_recall = token_recall(query_tokens, &meaningful_tokens(&page.title_normalised));
    let heading_recall = token_recall(query_tokens, &meaningful_tokens(&page.headings_normalised));
    let keyword_recall = token_recall(query_tokens, &meaningful_tokens(&page.keywords_normalised));
    let content_recall = token_recall(query_tokens, &page.all_tokens);
    let exact_title_bonus = if page.title_normalised.contains(query) {
        0.12
    } else {
        0.0
    };
    let exact_heading_bonus = if page.headings_normalised.contains(query) {
        0.08
    } else {
        0.0
    };
    let semantic_title = jaro_winkler(query, &page.title_normalised) * 0.10;

    (title_recall * 0.32
        + heading_recall * 0.24
        + keyword_recall * 0.18
        + content_recall * 0.16
        + semantic_title
        + exact_title_bonus
        + exact_heading_bonus)
        .clamp(0.0, 1.0)
}

fn opportunity_score(
    impressions: f64,
    position: f64,
    ctr: f64,
    expected_ctr: f64,
    coverage: f64,
    intent: SearchIntent,
    gap_types: &BTreeSet<GapType>,
) -> f64 {
    let demand = ((impressions + 1.0).ln() / 10.0).clamp(0.0, 1.0) * 32.0;
    let ranking_potential = if (8.0..=20.0).contains(&position) {
        24.0
    } else if (4.0..8.0).contains(&position) {
        16.0
    } else if (20.0..=40.0).contains(&position) {
        14.0
    } else if position > 40.0 {
        8.0
    } else {
        5.0
    };
    let ctr_gap = if expected_ctr > 0.0 {
        ((expected_ctr - ctr).max(0.0) / expected_ctr).clamp(0.0, 1.0) * 18.0
    } else {
        0.0
    };
    let coverage_gap = (1.0 - coverage).clamp(0.0, 1.0) * 16.0;
    let intent_bonus = match intent {
        SearchIntent::Transactional => 10.0,
        SearchIntent::Commercial => 8.0,
        SearchIntent::Local => 7.0,
        SearchIntent::Mixed => 6.0,
        SearchIntent::Informational => 4.0,
        SearchIntent::Navigational => 2.0,
    };
    let gap_bonus = if gap_types.contains(&GapType::NoRelevantLandingPage) {
        5.0
    } else if gap_types.contains(&GapType::UnderperformingSubtopic) {
        3.0
    } else {
        0.0
    };

    (demand + ranking_potential + ctr_gap + coverage_gap + intent_bonus + gap_bonus)
        .clamp(0.0, 100.0)
}

fn query_similarity(left: &str, right: &str) -> f64 {
    let left_tokens = meaningful_tokens(left);
    let right_tokens = meaningful_tokens(right);
    let jaccard = token_jaccard(&left_tokens, &right_tokens);
    let string_similarity = jaro_winkler(left, right);
    let shared_core_bonus = if left_tokens.intersection(&right_tokens).count() >= 2 {
        0.08
    } else {
        0.0
    };

    (jaccard * 0.68 + string_similarity * 0.32 + shared_core_bonus).clamp(0.0, 1.0)
}

fn classify_intent(query: &str) -> SearchIntent {
    let tokens = meaningful_tokens(query);
    let normalised = normalise_text(query);

    let transactional = contains_any(
        &normalised,
        &[
            "buy", "book", "hire", "order", "quote", "pricing", "price", "cost", "download",
            "subscribe", "appointment", "service",
        ],
    );
    let commercial = contains_any(
        &normalised,
        &[
            "best", "top", "review", "reviews", "compare", "comparison", "versus", "vs",
            "alternative", "company", "provider", "agency", "software", "tool",
        ],
    );
    let informational = is_question_query(&normalised)
        || contains_any(
            &normalised,
            &[
                "guide", "tips", "example", "examples", "meaning", "definition", "checklist",
                "ideas", "benefits",
            ],
        );
    let local = contains_any(
        &normalised,
        &["near me", "nearby", "local", "in my area"],
    ) || tokens.contains("london")
        || tokens.contains("sydney")
        || tokens.contains("melbourne")
        || tokens.contains("brisbane")
        || tokens.contains("perth");
    let navigational = contains_any(&normalised, &["login", "sign in", "contact", "official site"]);

    let active = [transactional, commercial, informational, local, navigational]
        .into_iter()
        .filter(|value| *value)
        .count();
    if active > 1 {
        SearchIntent::Mixed
    } else if transactional {
        SearchIntent::Transactional
    } else if commercial {
        SearchIntent::Commercial
    } else if local {
        SearchIntent::Local
    } else if navigational {
        SearchIntent::Navigational
    } else {
        SearchIntent::Informational
    }
}

fn dominant_intent(queries: &[QueryOpportunity]) -> SearchIntent {
    let mut totals: HashMap<SearchIntent, f64> = HashMap::new();
    for query in queries {
        *totals.entry(query.intent).or_default() += query.impressions.max(1.0);
    }
    totals
        .into_iter()
        .max_by(|left, right| left.1.partial_cmp(&right.1).unwrap_or(Ordering::Equal))
        .map(|(intent, _)| intent)
        .unwrap_or(SearchIntent::Informational)
}

fn dominant_page(queries: &[QueryOpportunity]) -> Option<String> {
    let mut totals: HashMap<String, f64> = HashMap::new();
    for query in queries {
        if let Some(page) = &query.mapped_page {
            *totals.entry(page.clone()).or_default() += query.impressions.max(1.0);
        }
    }
    totals
        .into_iter()
        .max_by(|left, right| left.1.partial_cmp(&right.1).unwrap_or(Ordering::Equal))
        .map(|(page, _)| page)
}

fn recommend_action(gaps: &[GapType], mapped_page: Option<&str>, primary: &str) -> String {
    if gaps.contains(&GapType::Cannibalisation) {
        return format!(
            "Review competing ranking URLs for '{primary}', select one primary landing page, consolidate overlapping content and strengthen internal linking to the preferred URL."
        );
    }
    if gaps.contains(&GapType::NoRelevantLandingPage) {
        return format!(
            "Create a dedicated page or blog asset for the '{primary}' cluster after confirming live SERP intent and checking for overlap with existing content."
        );
    }
    if gaps.contains(&GapType::HighImpressionLowCtr) {
        return format!(
            "Rework the title, description and opening copy on {} to match '{primary}' intent more clearly, then monitor CTR against the same position range.",
            mapped_page.unwrap_or("the mapped page")
        );
    }
    if gaps.contains(&GapType::StrikingDistance) {
        return format!(
            "Enhance {} with fuller coverage of '{primary}', relevant supporting sections, FAQs and contextual internal links.",
            mapped_page.unwrap_or("the mapped page")
        );
    }
    format!(
        "Review {} and expand its coverage of the '{primary}' cluster where the additions are genuinely useful to users.",
        mapped_page.unwrap_or("the best matching page")
    )
}

fn create_title(primary: &str, intent: SearchIntent) -> String {
    let title = title_case(primary);
    match intent {
        SearchIntent::Commercial => format!("{title}: Options, Costs and How to Choose"),
        SearchIntent::Transactional => format!("{title}: Services, Pricing and Next Steps"),
        SearchIntent::Local => format!("{title}: Local Services and What to Expect"),
        _ if is_question_query(primary) => title,
        _ => format!("{title}: A Practical Guide"),
    }
}

fn create_h2s(primary: &str, secondary: &[String], intent: SearchIntent) -> Vec<String> {
    let mut headings = Vec::new();
    let primary_title = title_case(primary);

    match intent {
        SearchIntent::Commercial | SearchIntent::Transactional | SearchIntent::Local => {
            headings.push(format!("What to Know About {primary_title}"));
            headings.push(format!("How to Choose the Right {primary_title}"));
            headings.push(format!("{primary_title} Costs and Key Considerations"));
        }
        _ => {
            headings.push(format!("What Is {primary_title}?"));
            headings.push(format!("How {primary_title} Works"));
            headings.push(format!("Common Questions About {primary_title}"));
        }
    }

    for keyword in secondary.iter().take(3) {
        let candidate = if is_question_query(keyword) {
            title_case(keyword)
        } else {
            format!("{}", title_case(keyword))
        };
        if !headings
            .iter()
            .any(|heading| normalise_text(heading) == normalise_text(&candidate))
        {
            headings.push(candidate);
        }
    }
    headings.truncate(6);
    headings
}

fn normalise_domain(domain: &str) -> Result<String, String> {
    let trimmed = domain.trim();
    let candidate = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = Url::parse(&candidate).map_err(|error| format!("Invalid domain: {error}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "Domain has no host".to_string())?;
    Ok(host.trim_start_matches("www.").to_ascii_lowercase())
}

fn normalise_url(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    match Url::parse(trimmed) {
        Ok(mut parsed) => {
            parsed.set_fragment(None);
            if parsed.path() != "/" {
                let cleaned = parsed.path().trim_end_matches('/').to_string();
                parsed.set_path(&cleaned);
            }
            parsed.to_string()
        }
        Err(_) => trimmed.trim_end_matches('/').to_string(),
    }
}

fn sanitise_settings(mut settings: ContentGapSettings) -> ContentGapSettings {
    settings.minimum_impressions = finite_or(settings.minimum_impressions, 20.0).max(0.0);
    settings.striking_distance_min = finite_or(settings.striking_distance_min, 8.0).max(1.0);
    settings.striking_distance_max = finite_or(settings.striking_distance_max, 20.0)
        .max(settings.striking_distance_min);
    settings.low_ctr_ratio = finite_or(settings.low_ctr_ratio, 0.60).clamp(0.05, 1.0);
    settings.minimum_page_coverage =
        finite_or(settings.minimum_page_coverage, 0.32).clamp(0.05, 0.95);
    settings.cluster_similarity =
        finite_or(settings.cluster_similarity, 0.56).clamp(0.25, 0.95);
    settings.max_secondary_keywords = settings.max_secondary_keywords.clamp(1, 50);
    settings.max_clusters = settings.max_clusters.clamp(1, 5_000);
    settings
}

fn meaningful_tokens(value: &str) -> HashSet<String> {
    let stopwords: HashSet<&'static str> = [
        "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "how", "in", "is",
        "it", "of", "on", "or", "that", "the", "this", "to", "with", "what", "when", "where",
        "which", "who", "why", "your", "you",
    ]
    .into_iter()
    .collect();

    normalise_text(value)
        .split_whitespace()
        .filter(|token| token.len() > 1 && !stopwords.contains(*token))
        .map(stem_token)
        .filter(|token| !token.is_empty())
        .collect()
}

fn stem_token(token: &str) -> String {
    let mut value = token.to_string();
    for suffix in ["ing", "ers", "ies", "ed", "es", "s"] {
        if value.len() > suffix.len() + 3 && value.ends_with(suffix) {
            value.truncate(value.len() - suffix.len());
            break;
        }
    }
    value
}

fn normalise_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character.is_whitespace() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn token_recall(query_tokens: &HashSet<String>, page_tokens: &HashSet<String>) -> f64 {
    if query_tokens.is_empty() {
        return 0.0;
    }
    query_tokens.intersection(page_tokens).count() as f64 / query_tokens.len() as f64
}

fn token_jaccard(left: &HashSet<String>, right: &HashSet<String>) -> f64 {
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    let union = left.union(right).count();
    if union == 0 {
        0.0
    } else {
        left.intersection(right).count() as f64 / union as f64
    }
}

fn sorted_ranking_pages(pages: &HashMap<String, f64>) -> Vec<String> {
    let mut values = pages.iter().collect::<Vec<_>>();
    values.sort_by(|left, right| right.1.partial_cmp(left.1).unwrap_or(Ordering::Equal));
    values.into_iter().map(|(url, _)| url.clone()).collect()
}

fn expected_ctr_for_position(position: f64) -> f64 {
    match position {
        value if value <= 1.5 => 0.28,
        value if value <= 2.5 => 0.15,
        value if value <= 3.5 => 0.10,
        value if value <= 4.5 => 0.075,
        value if value <= 5.5 => 0.055,
        value if value <= 7.5 => 0.040,
        value if value <= 10.5 => 0.025,
        value if value <= 20.5 => 0.012,
        value if value <= 40.5 => 0.006,
        _ => 0.003,
    }
}

fn is_question_query(query: &str) -> bool {
    let normalised = normalise_text(query);
    [
        "how ", "what ", "why ", "when ", "where ", "which ", "who ", "can ", "does ",
        "do ", "is ", "are ", "should ", "will ",
    ]
    .iter()
    .any(|prefix| normalised.starts_with(prefix))
        || query.trim_end().ends_with('?')
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        if needle.contains(' ') {
            haystack.contains(needle)
        } else {
            haystack.split_whitespace().any(|token| token == *needle)
        }
    })
}

fn create_slug(value: &str) -> String {
    normalise_text(value)
        .split_whitespace()
        .take(10)
        .collect::<Vec<_>>()
        .join("-")
}

fn title_case(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn weighted_average<I>(values: I) -> f64
where
    I: IntoIterator<Item = (f64, f64)>,
{
    let mut weighted_sum = 0.0;
    let mut total_weight = 0.0;
    for (value, weight) in values {
        if value.is_finite() && weight.is_finite() && weight > 0.0 {
            weighted_sum += value * weight;
            total_weight += weight;
        }
    }
    safe_ratio(weighted_sum, total_weight)
}

fn safe_ratio(numerator: f64, denominator: f64) -> f64 {
    if denominator.is_finite() && denominator > 0.0 && numerator.is_finite() {
        numerator / denominator
    } else {
        0.0
    }
}

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

fn round(value: f64, decimal_places: u32) -> f64 {
    let factor = 10_f64.powi(decimal_places as i32);
    (value * factor).round() / factor
}

fn compare_query_opportunities(
    left: &QueryOpportunity,
    right: &QueryOpportunity,
) -> Ordering {
    right
        .opportunity_score
        .partial_cmp(&left.opportunity_score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| {
            right
                .impressions
                .partial_cmp(&left.impressions)
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| left.query.cmp(&right.query))
}

fn intent_label(intent: SearchIntent) -> &'static str {
    match intent {
        SearchIntent::Informational => "informational",
        SearchIntent::Commercial => "commercial",
        SearchIntent::Transactional => "transactional",
        SearchIntent::Local => "local",
        SearchIntent::Navigational => "navigational",
        SearchIntent::Mixed => "mixed",
    }
}

fn gap_type_label(gap: GapType) -> &'static str {
    match gap {
        GapType::NoRelevantLandingPage => "no_relevant_landing_page",
        GapType::StrikingDistance => "striking_distance",
        GapType::HighImpressionLowCtr => "high_impression_low_ctr",
        GapType::UnderperformingSubtopic => "underperforming_subtopic",
        GapType::AccidentalRanking => "accidental_ranking",
        GapType::Cannibalisation => "cannibalisation",
        GapType::LongTailOpportunity => "long_tail_opportunity",
        GapType::QuestionOpportunity => "question_opportunity",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clusters_related_queries() {
        let settings = ContentGapSettings::default();
        assert!(query_similarity("seo audit tool", "website seo audit tools") >= 0.56);
        assert!(query_similarity("seo audit tool", "plumbing services london") < 0.56);
        assert_eq!(settings.minimum_impressions, 20.0);
    }

    #[test]
    fn identifies_a_striking_distance_gap() {
        let report = analyse_content_gaps(
            "example.com".to_string(),
            vec![GscQueryInput {
                query: "technical seo audit".to_string(),
                page: Some("https://example.com/seo".to_string()),
                clicks: 2.0,
                impressions: 500.0,
                ctr: 0.004,
                position: 12.0,
            }],
            vec![CrawledPageInput {
                url: "https://example.com/seo".to_string(),
                title: "SEO Services".to_string(),
                content: "General search engine optimisation services.".to_string(),
                ..CrawledPageInput::default()
            }],
            ContentGapSettings::default(),
        )
        .expect("analysis should succeed");

        let query = &report.clusters[0].queries[0];
        assert!(query.gap_types.contains(&GapType::StrikingDistance));
        assert!(query.gap_types.contains(&GapType::HighImpressionLowCtr));
    }

    #[test]
    fn normalises_ctr_from_clicks_and_impressions() {
        let (aggregated, _) = aggregate_queries(vec![GscQueryInput {
            query: "seo audit".to_string(),
            page: None,
            clicks: 10.0,
            impressions: 100.0,
            ctr: 10.0,
            position: 5.0,
        }]);
        let query = aggregated.get("seo audit").expect("query should exist");
        assert_eq!(safe_ratio(query.clicks, query.impressions), 0.1);
    }
}
