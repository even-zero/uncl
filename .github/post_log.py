import json, os, urllib.request

log = open('/tmp/build.log', errors='replace').read()[-55000:]
data = json.dumps({"body": "```\n" + log + "\n```"}).encode()
url = f"https://api.github.com/repos/{os.environ['GH_REPO']}/issues/{os.environ.get('PR', '51')}/comments"
req = urllib.request.Request(url, data=data, headers={
    "Authorization": f"Bearer {os.environ['GH_TOKEN']}",
    "Accept": "application/vnd.github+json",
    "User-Agent": "uncl-ci",
})
try:
    urllib.request.urlopen(req).read()
    print("posted the build log")
except Exception as err:
    print("could not post the log:", err)
