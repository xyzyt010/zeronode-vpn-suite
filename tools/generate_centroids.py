import json

def generate_centroids():
    with open('apps/client/assets/globe/countries_50m.geojson', 'r', encoding='utf-8') as f:
        data = json.load(f)
    
    centroids = {}
    for feature in data.get('features', []):
        props = feature.get('properties', {})
        iso = props.get('ISO_A2')
        name = props.get('NAME')
        if not iso or iso == '-99':
            continue
            
        geom = feature.get('geometry')
        if not geom: continue
        
        # very rough centroid: average of all coords
        coords = []
        def extract(c):
            if isinstance(c[0], (int, float)):
                coords.append(c)
            else:
                for x in c: extract(x)
        extract(geom.get('coordinates', []))
        
        if not coords: continue
        lat = sum(c[1] for c in coords) / len(coords)
        lng = sum(c[0] for c in coords) / len(coords)
        
        centroids[iso] = {
            "name": name,
            "lat": lat,
            "lng": lng
        }
        
    with open('apps/client/assets/globe/country_centroids.json', 'w', encoding='utf-8') as f:
        json.dump(centroids, f, indent=2)

if __name__ == "__main__":
    generate_centroids()
