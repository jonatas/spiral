import csv
import random
from datetime import datetime, timedelta

def generate_clickbench_sample(filename, num_rows=100000):
    columns = [
        "WatchID", "JavaEnable", "Title", "GoodEvent", "EventTime", "EventDate",
        "CounterID", "ClientIP", "RegionID", "UserID", "CounterClass", "OS",
        "UserAgent", "URL", "Referer", "IsRefresh", "RefererCategoryID",
        "RefererRegionID", "URLCategoryID", "URLRegionID", "ResolutionWidth",
        "ResolutionHeight", "ResolutionDepth", "FlashMajor", "FlashMinor",
        "FlashMinor2", "NetMajor", "NetMinor", "UserAgentMajor", "UserAgentMinor",
        "CookieEnable", "JavascriptEnable", "IsMobile", "MobilePhone",
        "MobilePhoneModel", "Params", "IPNetworkID", "TraficSourceID",
        "SearchEngineID", "SearchPhrase", "AdvEngineID", "IsArtifical",
        "WindowClientWidth", "WindowClientHeight", "ClientTimeZone",
        "ClientEventTime", "SilverlightVersion1", "SilverlightVersion2",
        "SilverlightVersion3", "SilverlightVersion4", "PageCharset",
        "CodeVersion", "IsLink", "IsDownload", "IsNotBounce", "FUniqID",
        "OriginalURL", "HID", "IsOldCounter", "IsEvent", "IsParameter",
        "DontCountHits", "WithHash", "HitColor", "LocalEventTime", "Age",
        "Sex", "Income", "Interests", "Robotness", "RemoteIP", "WindowName",
        "OpenerName", "HistoryLength", "BrowserLanguage", "BrowserCountry",
        "SocialNetwork", "SocialAction", "HTTPError", "SendTiming", "DNSTiming",
        "ConnectTiming", "ResponseStartTiming", "ResponseEndTiming",
        "FetchTiming", "SocialSourceNetworkID", "SocialSourcePage",
        "ParamPrice", "ParamOrderID", "ParamCurrency", "ParamCurrencyID",
        "OpenstatServiceName", "OpenstatCampaignID", "OpenstatAdID",
        "OpenstatSourceID", "UTMSource", "UTMMedium", "UTMCampaign",
        "UTMContent", "UTMTerm", "FromTag", "HasGCLID", "RefererHash",
        "URLHash", "CLID"
    ]

    start_date = datetime(2013, 7, 1)
    
    with open(filename, 'w', newline='') as csvfile:
        writer = csv.writer(csvfile, delimiter='\t')
        # writer.writerow(columns) # ClickBench usually loads without header if using COPY
        
        for i in range(num_rows):
            event_time = start_date + timedelta(seconds=random.randint(0, 86400 * 365 * 5))
            event_date = event_time.date()
            
            row = [
                random.getrandbits(63), # WatchID
                random.randint(0, 1),   # JavaEnable
                f"Title {random.randint(0, 1000)}", # Title
                random.randint(0, 1),   # GoodEvent
                event_time.strftime('%Y-%m-%d %H:%M:%S'), # EventTime
                event_date.strftime('%Y-%m-%d'), # EventDate
                random.randint(0, 1000), # CounterID
                random.getrandbits(31), # ClientIP
                random.randint(0, 1000), # RegionID
                random.getrandbits(63), # UserID
                random.randint(0, 10),  # CounterClass
                random.randint(0, 20),  # OS
                random.randint(0, 100), # UserAgent
                f"http://example.com/{random.randint(0, 1000)}", # URL
                f"http://referer.com/{random.randint(0, 1000)}", # Referer
                random.randint(0, 1),   # IsRefresh
                random.randint(0, 10),  # RefererCategoryID
                random.randint(0, 1000), # RefererRegionID
                random.randint(0, 10),  # URLCategoryID
                random.randint(0, 1000), # URLRegionID
                random.randint(800, 2560), # ResolutionWidth
                random.randint(600, 1440), # ResolutionHeight
                random.randint(8, 32),   # ResolutionDepth
                random.randint(0, 20),   # FlashMajor
                random.randint(0, 100),  # FlashMinor
                f"Minor2 {random.randint(0, 100)}", # FlashMinor2
                random.randint(0, 10),   # NetMajor
                random.randint(0, 100),  # NetMinor
                random.randint(0, 100),  # UserAgentMajor
                f"Minor {random.randint(0, 100)}", # UserAgentMinor
                random.randint(0, 1),    # CookieEnable
                random.randint(0, 1),    # JavascriptEnable
                random.randint(0, 1),    # IsMobile
                random.randint(0, 1),    # MobilePhone
                f"Model {random.randint(0, 100)}", # MobilePhoneModel
                " ", # Params
                random.getrandbits(31), # IPNetworkID
                random.randint(0, 10),  # TraficSourceID
                random.randint(0, 10),  # SearchEngineID
                f"Search Phrase {random.randint(0, 100)}", # SearchPhrase
                random.randint(0, 10),  # AdvEngineID
                random.randint(0, 1),   # IsArtifical
                random.randint(800, 2560), # WindowClientWidth
                random.randint(600, 1440), # WindowClientHeight
                random.randint(-12, 12), # ClientTimeZone
                event_time.strftime('%Y-%m-%d %H:%M:%S'), # ClientEventTime
                random.randint(0, 10),  # SilverlightVersion1
                random.randint(0, 10),  # SilverlightVersion2
                random.randint(0, 1000), # SilverlightVersion3
                random.randint(0, 10),  # SilverlightVersion4
                "UTF-8", # PageCharset
                random.randint(0, 1000), # CodeVersion
                random.randint(0, 1),    # IsLink
                random.randint(0, 1),    # IsDownload
                random.randint(0, 1),    # IsNotBounce
                random.getrandbits(63),  # FUniqID
                f"http://original.com/{random.randint(0, 1000)}", # OriginalURL
                random.getrandbits(31),  # HID
                random.randint(0, 1),    # IsOldCounter
                random.randint(0, 1),    # IsEvent
                random.randint(0, 1),    # IsParameter
                random.randint(0, 1),    # DontCountHits
                random.randint(0, 1),    # WithHash
                ' ', # HitColor
                event_time.strftime('%Y-%m-%d %H:%M:%S'), # LocalEventTime
                random.randint(0, 100),  # Age
                random.randint(0, 2),    # Sex
                random.randint(0, 10),   # Income
                random.randint(0, 100),  # Interests
                random.randint(0, 10),   # Robotness
                random.getrandbits(31),  # RemoteIP
                random.randint(0, 1000), # WindowName
                random.randint(0, 1000), # OpenerName
                random.randint(0, 100),  # HistoryLength
                "en", # BrowserLanguage
                "US", # BrowserCountry
                " ", # SocialNetwork
                " ", # SocialAction
                random.randint(0, 500), # HTTPError
                random.randint(0, 1000), # SendTiming
                random.randint(0, 1000), # DNSTiming
                random.randint(0, 1000), # ConnectTiming
                random.randint(0, 1000), # ResponseStartTiming
                random.randint(0, 1000), # ResponseEndTiming
                random.randint(0, 1000), # FetchTiming
                random.randint(0, 10),   # SocialSourceNetworkID
                " ", # SocialSourcePage
                random.getrandbits(31), # ParamPrice
                " ", # ParamOrderID
                " ", # ParamCurrency
                random.randint(0, 10),  # ParamCurrencyID
                " ", # OpenstatServiceName
                " ", # OpenstatCampaignID
                " ", # OpenstatAdID
                " ", # OpenstatSourceID
                " ", # UTMSource
                " ", # UTMMedium
                " ", # UTMCampaign
                " ", # UTMContent
                " ", # UTMTerm
                " ", # FromTag
                random.randint(0, 1),   # HasGCLID
                random.getrandbits(63), # RefererHash
                random.getrandbits(63), # URLHash
                random.getrandbits(31)  # CLID
            ]
            writer.writerow(row)

if __name__ == "__main__":
    generate_clickbench_sample("clickbench/hits.csv", num_rows=10000000)
