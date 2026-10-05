require 'json'
require 'net/http'
require 'time'
require 'uri'

MAX_RESPONSE = 4 * 1024 * 1024
KINDS = {
  'image' => { 'pipeline' => 'text-to-image', 'search' => 'flux1-schnell' },
  'video' => { 'pipeline' => 'text-to-video', 'search' => 'wan2.1-t2v' }
}.freeze

def repository?(value)
  value.is_a?(String) &&
    value.match?(/\A[A-Za-z0-9][A-Za-z0-9._-]*\/[A-Za-z0-9][A-Za-z0-9._-]*\z/)
end

def fetch_json(path, query = nil)
  raise 'unsafe Hugging Face path' unless path.start_with?('/api/models/') || path == '/api/models'
  uri = URI::HTTPS.build(host: 'huggingface.co', path: path, query: query && URI.encode_www_form(query))
  request = Net::HTTP::Get.new(uri)
  request['User-Agent'] = 'RigSpark-Generation-Enrichment/1.0'
  token = ENV['HF_TOKEN']
  if token && !token.empty?
    raise 'invalid HF_TOKEN' if token.bytesize > 4096 || token.match?(/[[:cntrl:]]/)
    request['Authorization'] = "Bearer #{token}"
  end
  body = ''.b
  status = nil
  Net::HTTP.start(uri.host, 443, nil, use_ssl: true, open_timeout: 10, read_timeout: 60) do |http|
    http.request(request) do |response|
      status = response.code.to_i
      response.read_body do |chunk|
        raise 'Hugging Face response exceeds 4 MiB' if body.bytesize + chunk.bytesize > MAX_RESPONSE
        body << chunk
      end
    end
  end
  raise "Hugging Face HTTP #{status}" unless status == 200
  JSON.parse(body)
end

def pinned_metadata(repo, cache)
  cache[repo] ||= begin
    current = fetch_json("/api/models/#{repo}")
    revision = current.fetch('sha')
    raise 'invalid immutable revision' unless revision.match?(/\A[0-9a-f]{40}\z/)
    metadata = fetch_json("/api/models/#{repo}/revision/#{revision}", { 'blobs' => 'true' })
    raise 'revision changed during collection' unless metadata['sha'] == revision
    [revision, metadata]
  end
end

def enriched_model(model, cache)
  copy = Marshal.load(Marshal.dump(model))
  source_repo = URI(copy.fetch('source')).path.delete_prefix('/')
  raise 'invalid source repository coordinates' unless repository?(source_repo)
  _, source_metadata = pinned_metadata(source_repo, cache)
  upstream_license = source_metadata.dig('cardData', 'license')
  raise 'source license does not match the allowlisted catalog license' unless upstream_license == copy.fetch('license')
  copy.fetch('files').each do |file|
    repo = file.fetch('repo')
    raise 'invalid repository coordinates' unless repository?(repo)
    revision, metadata = pinned_metadata(repo, cache)
    sibling = metadata.fetch('siblings').find { |entry| entry['rfilename'] == file.fetch('file') }
    raise "weight file missing: #{repo}/#{file['file']}" unless sibling
    lfs = sibling.fetch('lfs')
    digest = lfs.fetch('oid').delete_prefix('sha256:')
    bytes = lfs.fetch('size')
    raise 'invalid weight digest' unless digest.match?(/\A[0-9a-f]{64}\z/)
    raise 'invalid weight size' unless bytes.is_a?(Integer) && bytes.positive?
    file['revision'] = revision
    file['sha256'] = digest
    file['bytes'] = bytes
  end
  copy
end

if ARGV == ['--self-test']
  raise 'accepted unsafe repository' if repository?('../bad')
  raise 'rejected valid repository' unless repository?('Comfy-Org/flux1-schnell')
  raise 'missing kind configuration' unless KINDS.keys == %w[image video]
  puts 'PASS: generation Hugging Face collector bounds'
  exit
end

abort 'Usage: ruby scripts/generation-hf-enrich.rb KIND CATALOG CANDIDATES REPORT' unless ARGV.length == 4
kind, catalog_path, candidates_path, report_path = ARGV
config = KINDS[kind] or abort 'KIND must be image or video'
catalog = JSON.parse(File.read(catalog_path))
models = catalog.fetch('models').select { |model| model['kind'] == kind }
abort "generation catalog has no #{kind} models" if models.empty?

cache = {}
candidates = models.map { |model| enriched_model(model, cache) }
known_sources = models.map { |model| URI(model.fetch('source')).path.delete_prefix('/') }
discovered = fetch_json('/api/models', {
  'search' => config['search'],
  'pipeline_tag' => config['pipeline'],
  'limit' => '100',
  'full' => 'true'
})
raise 'invalid discovery response' unless discovered.is_a?(Array) && discovered.length <= 100
unknown = discovered.filter_map do |entry|
  repo = entry['id']
  next if !repository?(repo) || known_sources.include?(repo)
  {
    'repository' => repo,
    'pipelineTag' => entry['pipeline_tag'],
    'license' => entry.dig('cardData', 'license'),
    'reason' => 'not auto-added: no reviewed built-in workflow mapping'
  }
end

File.write(candidates_path, JSON.pretty_generate(candidates) + "\n")
File.write(report_path, JSON.pretty_generate({
  'schemaVersion' => 1,
  'kind' => kind,
  'generatedAt' => Time.now.utc.iso8601,
  'requiresReview' => true,
  'knownCandidates' => candidates.map { |model| model['id'] },
  'unknownCandidates' => unknown
}) + "\n")
warn "generation-hf-enrich: kind=#{kind} known=#{candidates.length} unknown=#{unknown.length}"
