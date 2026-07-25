#include "wrapper.hpp"
#include "clipper.hpp"
#include <iostream>
#include <queue>

using ClipperLib::IntPoint;
using ClipperLib::Paths;
using ClipperLib::PolyNode;
using ClipperLib::PolyTree;
using ClipperLib::PolyTreeToPaths;
using ClipperLib::ReversePath;

ClipperLib::Path get_path(const Path &path)
{
    ClipperLib::Path clipper_path;
    clipper_path.reserve(path.vertices_count);

    for (size_t i = 0; i < path.vertices_count; ++i)
    {
        clipper_path.push_back(IntPoint(path.vertices[i][0], path.vertices[i][1]));
    }
    return clipper_path;
}

std::pair<Paths, std::vector<bool> > get_polygon_paths(const Polygon &polygon)
{
    Paths paths;
    paths.reserve(polygon.paths_count);

    std::vector<bool> closed;
    closed.reserve(polygon.paths_count);

    for (size_t i = 0; i < polygon.paths_count; ++i)
    {
        paths.push_back(get_path(polygon.paths[i]));
        closed.push_back(polygon.paths[i].closed);
    }
    return std::make_pair(paths, closed);
}

void add_paths(ClipperLib::Clipper &c, const Polygons &polygons)
{
    for (size_t i = 0; i < polygons.polygons_count; ++i)
    {
        auto &polygon = polygons.polygons[i];
        auto paths_closed = get_polygon_paths(polygon);
        Paths &paths = paths_closed.first;
        std::vector<bool> &closed = paths_closed.second;
        for (size_t i = 0; i < paths.size(); ++i)
        {
            c.AddPath(paths[i], ClipperLib::PolyType(polygon.type), closed[i]);
        }
    }
}

void add_paths(ClipperLib::ClipperOffset &c, JoinType join_type, EndType end_type, const Polygons &polygons)
{
    for (size_t i = 0; i < polygons.polygons_count; ++i)
    {
        auto &polygon = polygons.polygons[i];
        auto paths_closed = get_polygon_paths(polygon);
        Paths &paths = paths_closed.first;
        c.AddPaths(paths, ClipperLib::JoinType(join_type), ClipperLib::EndType(end_type));
    }
}

Path get_path_from_node(const PolyNode &node)
{
    Path path;
    path.vertices_count = node.Contour.size();
    path.vertices = new Vertice[path.vertices_count];
    path.closed = !node.IsOpen();
    for (size_t i = 0; i < path.vertices_count; ++i)
    {
        path.vertices[i][0] = node.Contour[i].X;
        path.vertices[i][1] = node.Contour[i].Y;
    }
    return path;
}

Path get_path_from_closed_path(ClipperLib::Path &clipper_path)
{
    Path path;
    path.vertices_count = clipper_path.size();
    path.vertices = new Vertice[path.vertices_count];
    path.closed = true;
    for (size_t i = 0; i < path.vertices_count; ++i)
    {
        path.vertices[i][0] = clipper_path[i].X;
        path.vertices[i][1] = clipper_path[i].Y;
    }
    return path;
}

ClipperLib::Paths get_closed_paths_from_polygons(Polygons polygons)
{
    Paths paths;

    for (size_t i = 0; i < polygons.polygons_count; ++i)
    {
        auto &polygon = polygons.polygons[i];
        auto paths_closed = get_polygon_paths(polygon);
        Paths &next_paths = paths_closed.first;

        paths.reserve(paths.size() + next_paths.size());
        paths.insert(paths.end(), next_paths.begin(), next_paths.end());
    }

    return paths;
}

Polygons get_polygons_from_closed_paths(ClipperLib::Paths &closed_paths)
{
    std::vector<Polygon> polygon_vector;

    for (size_t i = 0; i < closed_paths.size(); ++i)
    {
        Polygon polygon;
        polygon.type = ptSubject;
        polygon.paths_count = 1;
        polygon.paths = new Path[1];
        polygon.paths[0] = get_path_from_closed_path(closed_paths[i]);
        polygon_vector.push_back(polygon);
    }

    Polygons polygons;
    polygons.polygons_count = polygon_vector.size();
    polygons.polygons = new Polygon[polygons.polygons_count];
    std::copy(polygon_vector.begin(), polygon_vector.end(), polygons.polygons);

    return polygons;
}

Polygon get_polygon_from_node(
    const PolyNode *node, std::queue<const PolyNode *> &node_queue)
{
    Polygon polygon;
    polygon.type = ptSubject;
    polygon.paths_count = node->ChildCount() + 1;
    polygon.paths = new Path[polygon.paths_count];
    polygon.paths[0] = get_path_from_node(*node);
    for (int i = 0; i < node->ChildCount(); ++i)
    {
        auto child = node->Childs[i];
        polygon.paths[i + 1] = get_path_from_node(*child);
        for (const auto grand_child : child->Childs)
        {
            node_queue.push(grand_child);
        }
    }
    return polygon;
}

Polygons get_polygons_from_tree(const PolyTree &tree)
{
    std::queue<const PolyNode *> node_queue;
    std::vector<Polygon> polygon_vector;

    for (const auto node : tree.Childs)
    {
        node_queue.push(node);
    }

    while (!node_queue.empty())
    {
        const auto node = node_queue.front();
        node_queue.pop();
        polygon_vector.push_back(get_polygon_from_node(node, node_queue));
    }

    Polygons polygons;
    polygons.polygons_count = polygon_vector.size();
    polygons.polygons = new Polygon[polygons.polygons_count];
    std::copy(polygon_vector.begin(), polygon_vector.end(), polygons.polygons);

    return polygons;
}

Polygons execute(
    ClipType clip_type,
    Polygons polygons,
    PolyFillType subject_fill_type,
    PolyFillType clip_fill_type)
{
    ClipperLib::Clipper c;
    add_paths(c, polygons);
    PolyTree solution;
    c.Execute(
        ClipperLib::ClipType(clip_type), solution,
        ClipperLib::PolyFillType(subject_fill_type),
        ClipperLib::PolyFillType(clip_fill_type));
    return get_polygons_from_tree(solution);
}

Polygons offset_simplify_clean(
    Polygons polygons,
    double miter_limit,
    double round_precision,
    JoinType join_type,
    EndType end_type,
    double delta,
    PolyFillType fill_type,
    double distance)
{
    // Fix orientation of closed paths
    ClipperLib::Clipper c_clipper;
    add_paths(c_clipper, polygons);
    PolyTree solution_clipper;
    c_clipper.Execute(
        ClipperLib::ClipType::ctDifference,
        solution_clipper,
        ClipperLib::PolyFillType::pftEvenOdd,
        ClipperLib::PolyFillType::pftEvenOdd);
    Paths paths_orient;
    PolyTreeToPaths(solution_clipper, paths_orient);

    // Offset closed paths
    ClipperLib::ClipperOffset c_off(miter_limit, round_precision);
    c_off.AddPaths(paths_orient, ClipperLib::JoinType(join_type), ClipperLib::EndType(end_type));
    PolyTree solution_off;
    c_off.Execute(solution_off, delta);
    Paths paths_off;
    PolyTreeToPaths(solution_off, paths_off);

    // Simplify overlapping or touching paths
    Paths paths_simpl;
    SimplifyPolygons(paths_off, paths_simpl, ClipperLib::PolyFillType(fill_type));

    // Clean polygons
    Paths paths_clean;
    CleanPolygons(paths_simpl, paths_clean, distance);

    return get_polygons_from_closed_paths(paths_clean);
}

Polygons offset(
    double miter_limit,
    double round_precision,
    JoinType join_type,
    EndType end_type,
    Polygons polygons,
    double delta)
{
    // Fix orientation of closed paths
    ClipperLib::Clipper c_clipper;
    add_paths(c_clipper, polygons);
    PolyTree solution_clipper;
    c_clipper.Execute(
        ClipperLib::ClipType::ctDifference,
        solution_clipper,
        ClipperLib::PolyFillType::pftEvenOdd,
        ClipperLib::PolyFillType::pftEvenOdd);
    Paths paths;
    PolyTreeToPaths(solution_clipper, paths);
    // Offset closed paths
    ClipperLib::ClipperOffset c_off(miter_limit, round_precision);
    c_off.AddPaths(paths, ClipperLib::JoinType(join_type), ClipperLib::EndType(end_type));
    PolyTree solution_off;
    c_off.Execute(solution_off, delta);

    return get_polygons_from_tree(solution_off);
}

Polygons simplify(Polygons polygons, PolyFillType fill_type)
{
    Paths paths = get_closed_paths_from_polygons(polygons);
    SimplifyPolygons(paths, ClipperLib::PolyFillType(fill_type));
    return get_polygons_from_closed_paths(paths);
}

Polygons clean(Polygons polygons, double distance)
{
    Paths paths = get_closed_paths_from_polygons(polygons);
    CleanPolygons(paths, distance);
    return get_polygons_from_closed_paths(paths);
}

void free_path(Path path)
{
    delete[] path.vertices;
}

void free_polygon(Polygon polygon)
{
    for (size_t i = 0; i < polygon.paths_count; ++i)
    {
        free_path(polygon.paths[i]);
    }
    delete[] polygon.paths;
}

void free_polygons(Polygons polygons)
{
    for (size_t i = 0; i < polygons.polygons_count; ++i)
    {
        free_polygon(polygons.polygons[i]);
    }
    delete[] polygons.polygons;
}
