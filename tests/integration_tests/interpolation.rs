use itertools::{izip, Itertools};
use matrixcompare::assert_matrix_eq;
use nalgebra::{DefaultAllocator, DVectorSlice, OMatrix, OVector, Point2, U1, U2, vector, Vector1, Vector2};
use fenris::assembly::buffers::{BufferUpdate, InterpolationBuffer};
use fenris::io::vtk::FiniteElementMeshDataSetBuilder;
use fenris::mesh::procedural::create_unit_square_uniform_tri_mesh_2d;
use fenris::mesh::TriangleMesh2d;
use fenris::space::{InterpolateGradientInSpace, SpatiallyIndexed};
use fenris::util::global_vector_from_point_fn;
use fenris::{quadrature, SmallDim};
use fenris_traits::allocators::{BiDimAllocator, TriDimAllocator};
use crate::integration_tests::data_output_path;

fn u_scalar(p: &Point2<f64>) -> Vector1<f64> {
    let (x, y) = (p.x, p.y);
    Vector1::new((x.cos() + y.sin()) * x.powi(2))
}

fn u_vector(p: &Point2<f64>) -> Vector2<f64> {
    let (x, y) = (p.x, p.y);
    vector![(x.cos() + y.sin()) * x.powi(2),
            (x.powi(2) + 0.5).ln() * (y.powi(2) + 0.25).ln() + x * y + 3.0]
}

struct ExpectedInterpolationTestValues<SolutionDim>
where
    SolutionDim: SmallDim,
    DefaultAllocator: BiDimAllocator<f64, U2, SolutionDim>,
{
    u_expected: Vec<OVector<f64, SolutionDim>>,
    grad_u_expected: Vec<OMatrix<f64, U2, SolutionDim>>,
    u_interpolated: Vec<OVector<f64, SolutionDim>>,
    grad_u_interpolated: Vec<OMatrix<f64, U2, SolutionDim>>,
}

fn compute_expected_interpolation_test_values<'a, Space, SolutionDim>(
    space: &Space,
    quadrature_points: &[Point2<f64>],
    u_vec: impl Into<DVectorSlice<'a, f64>>,
) -> ExpectedInterpolationTestValues<SolutionDim>
where
    SolutionDim: SmallDim,
    Space: InterpolateGradientInSpace<f64, SolutionDim, GeometryDim=U2, ReferenceDim=U2>,
    DefaultAllocator: TriDimAllocator<f64, U2, U2, SolutionDim>
{
    let u_vec = u_vec.into();
    let mut interpolation_buffer = InterpolationBuffer::default();

    // For each element, compute interpolated value + gradient at quadrature points plus
    // the map to physical space. Then later we'll interpolate at these same points (in physical
    // space), so that we already know the correct answer.
    let (x_expected, u_expected, grad_u_expected): (Vec<_>, Vec<_>, Vec<_>) = (0 .. space.num_elements())
        .flat_map(|i| {
            let mut buffer = interpolation_buffer.prepare_element_in_space(i, space, u_vec, SolutionDim::dim());
            quadrature_points
                .iter()
                .map(|xi_j| {
                    buffer.update_reference_point(xi_j, BufferUpdate::Both);
                    let u_j: OVector<_, SolutionDim> = buffer.interpolate();
                    let grad_u_j_ref: OMatrix<_, Space::ReferenceDim, SolutionDim> = buffer.interpolate_ref_gradient();
                    let j_inv_t = buffer.element_reference_jacobian()
                        .try_inverse()
                        .unwrap()
                        .transpose();
                    let grad_u_j = j_inv_t * grad_u_j_ref;
                    let x_j = space.map_element_reference_coords(i, xi_j);
                    (x_j, u_j, grad_u_j)
                }).collect::<Vec<_>>()
        })
        .multiunzip();

    let mut u_interpolated = vec![OVector::<_, SolutionDim>::zeros(); x_expected.len()];
    let mut grad_u_interpolated = vec![OMatrix::<_, U2, SolutionDim>::zeros(); x_expected.len()];
    space.interpolate_at_points(&x_expected, DVectorSlice::from(&u_vec), &mut u_interpolated);
    space.interpolate_gradient_at_points(&x_expected, DVectorSlice::from(&u_vec), &mut grad_u_interpolated);

    ExpectedInterpolationTestValues {
        u_expected,
        grad_u_expected,
        u_interpolated,
        grad_u_interpolated,
    }
}

#[test]
fn spatially_indexed_interpolation_trimesh() {
    // We interpolate at (quadrature) points of a finite element space
    // in two ways:
    //  - by computing the values in reference coordinate space of each element
    //    (this forms the "expected" values)
    //  - by interpolating the quantity at the *physical* coordinates
    // This way we verify that the latter approach produces expected results.
    let mesh: TriangleMesh2d<f64> = create_unit_square_uniform_tri_mesh_2d(10);

    // Arbitrary scalar function u(p), where p is a 2-dimensional point
    let u_weights_scalar = global_vector_from_point_fn(mesh.vertices(), u_scalar);
    let u_weights_vector = global_vector_from_point_fn(mesh.vertices(), u_vector);
    let space = SpatiallyIndexed::from_space(mesh);

    let (_, interior_points) = quadrature::total_order::triangle::<f64>(4).unwrap();
    let interface_points = [
        // Points on the boundary of the reference element, which will be mapped to
        // the boundary of the physical element, and thus on an interface between
        // neighboring elements
        [-1.0, -1.0],
        [1.0, -1.0],
        [-1.0, 1.0],
        [-1.0, 0.5],
        [0.5, -1.0],
        [0.0, 0.0],
    ].map(Point2::from);

    // For debugging
    FiniteElementMeshDataSetBuilder::from_mesh(space.space())
        .try_export(data_output_path()
            .join("interpolation/spatially_indexed_interpolation_trimesh/mesh.vtu"))
        .unwrap();

    // For each element, compute interpolated value of quadrature points plus
    // the map to physical space. Then later we'll interpolate at these same points (in physical
    // space), so that we already know the correct answer.
    {
        // For interior quadrature points, we check both values and gradients of the scalar function
        let values = compute_expected_interpolation_test_values::<_, U1>(&space, &interior_points, &u_weights_scalar);
        let iter = izip!(values.u_interpolated, values.grad_u_interpolated, values.u_expected, values.grad_u_expected);
        for (u, grad_u, u_expected, grad_u_expected) in iter {
            assert_matrix_eq!(u, u_expected, comp = abs, tol = 1e-12);
            assert_matrix_eq!(grad_u, grad_u_expected, comp = abs, tol = 1e-12);
        }
    }

    {
        // For boundary points, we only check values since gradients are discontinuous
        // at element interfaces
        let values = compute_expected_interpolation_test_values::<_, U1>(&space, &interface_points, &u_weights_scalar);
        let iter = izip!(values.u_interpolated, values.u_expected);
        for (u, u_expected) in iter {
            assert_matrix_eq!(u, u_expected, comp = abs, tol = 1e-12);
        }
    }

    {
        // Repeat interior quadrature points for vector function
        let values = compute_expected_interpolation_test_values::<_, U2>(&space, &interior_points, &u_weights_vector);
        let iter = izip!(values.u_interpolated, values.grad_u_interpolated, values.u_expected, values.grad_u_expected);
        for (u, grad_u, u_expected, grad_u_expected) in iter {
            assert_matrix_eq!(u, u_expected, comp = abs, tol = 1e-12);
            assert_matrix_eq!(grad_u, grad_u_expected, comp = abs, tol = 1e-12);
        }
    }

    {
        // Repeat interface quadrature points for vector function
        let values = compute_expected_interpolation_test_values::<_, U2>(&space, &interface_points, &u_weights_vector);
        let iter = izip!(values.u_interpolated, values.u_expected);
        for (u, u_expected) in iter {
            assert_matrix_eq!(u, u_expected, comp = abs, tol = 1e-12);
        }
    }
}